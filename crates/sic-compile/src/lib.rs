//! Lowering from HIR to bytecode.
//!
//! Register allocation is deliberately trivial: local `n` becomes register `n`,
//! and a small scratch area above the locals holds call arguments. Anything
//! smarter would have to be justified by a measurement, and there is none yet.

use std::collections::HashMap;

use sic_bytecode::inst::{Inst, Op};
use sic_bytecode::program::*;
use sic_core::{BlockId, SourceMap, Span};
use sic_ir::hir::{
    BinOp, CallPolicy as HirCallPolicy, Const as HirConst, Hir, HirFunc, Inst as HirInst, InstKind,
    Term, Terminator, UnOp,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompileError {
    pub message: String,
}

impl std::fmt::Display for CompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl CompileError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

/// The limits the format imposes. Exceeding one is a compile error rather than
/// something the verifier has to catch later.
const MAX_REGISTERS: usize = 256;
const MAX_CONSTS: usize = u16::MAX as usize + 1;
const MAX_FUNCS: usize = 256;

pub fn compile(hir: &Hir, sources: &SourceMap) -> Result<Program, Vec<CompileError>> {
    let mut errors = Vec::new();

    if hir.funcs.len() > MAX_FUNCS {
        errors.push(CompileError::new(format!(
            "a module can hold at most {MAX_FUNCS} functions, found {}",
            hir.funcs.len()
        )));
    }

    let mut consts: Vec<Const> = hir.consts.iter().map(to_bytecode_const).collect();

    let mut types = TypeSection::new();
    let mut program = Program {
        debug: DebugInfo {
            sources: sources
                .files()
                .iter()
                .map(|f| f.name().to_string())
                .collect(),
            lines: Vec::new(),
        },
        ..Program::default()
    };

    program.caps = hir
        .caps
        .iter()
        .map(|c| CapDecl {
            name: c.name.clone(),
            kind: c.kind,
            constraints: c.constraint.clone(),
            pin: c.pin.clone(),
            args: c.args.clone(),
            repeatable: c.repeatable,
            delegable: c.delegable,
            dir: c.dir.clone(),
            env: c.env.clone(),
            params: c
                .params
                .iter()
                .map(|t| types.intern(*t, &hir.types))
                .collect(),
            ret_type: types.intern(c.ret, &hir.types),
        })
        .collect();

    for func in &hir.funcs {
        match FnCompile::new(func, &mut consts, &mut types, &hir.types).run() {
            Ok(compiled) => {
                let code_off = program.code.len() as u32;
                for (offset, span) in compiled.spans {
                    let file = sources.file_index(span.lo) as u32;
                    let pos = sources.line_col(span.lo);
                    program
                        .debug
                        .lines
                        .push((code_off + offset, file, pos.line, pos.col));
                }
                program.code.extend(compiled.code);
                for (offset, policy) in compiled.policies {
                    program.policies.push(PolicyEntry {
                        pc: code_off + offset,
                        attempts: policy.attempts.unwrap_or(1),
                        timeout_ms: policy.timeout_ms.unwrap_or(0),
                        budget: policy.budget.unwrap_or(0),
                        conversation: policy.conversation.unwrap_or(0),
                        tools: policy.tools.unwrap_or(0),
                        deadline_ms: policy.deadline_ms.unwrap_or(0),
                    });
                }
                program.funcs.push(FuncDef {
                    name: func.name.clone(),
                    params: func
                        .params
                        .iter()
                        .map(|p| types.intern(func.locals[p.index()], &hir.types))
                        .collect(),
                    reg_count: compiled.reg_count,
                    ret_type: types.intern(func.ret, &hir.types),
                    code_off,
                    code_len: program.code.len() as u32 - code_off,
                });
            }
            Err(mut errs) => errors.append(&mut errs),
        }
    }

    if consts.len() > MAX_CONSTS {
        errors.push(CompileError::new(format!(
            "a module can hold at most {MAX_CONSTS} constants, found {}",
            consts.len()
        )));
    }
    program.consts = consts;
    // Every task type a `SPAWN` produces has to be in the section, so the
    // verifier can name what `AWAIT` will give back.
    for func in &hir.funcs {
        for local in &func.locals {
            types.intern(*local, &hir.types);
        }
    }
    program.types = types.descs;

    if errors.is_empty() {
        Ok(program)
    } else {
        Err(errors)
    }
}

/// The level as one byte, which is what an ABC operand is.
///
/// The numbers are part of the file format, like an opcode's: a reader that
/// took `2` for `warn` would report the wrong thing about a run.
fn level_code(level: sic_ir::hir::LogLevel) -> u8 {
    use sic_ir::hir::LogLevel;
    match level {
        LogLevel::Debug => 0,
        LogLevel::Info => 1,
        LogLevel::Warn => 2,
        LogLevel::Error => 3,
    }
}

fn to_bytecode_const(c: &HirConst) -> Const {
    match c {
        HirConst::Unit => Const::Unit,
        HirConst::Bool(v) => Const::Bool(*v),
        HirConst::I64(v) => Const::I64(*v),
        HirConst::F64(v) => Const::F64(*v),
        HirConst::Str(s) => Const::Str(s.clone()),
    }
}

/// Adds a constant, reusing an existing entry when there is one.
fn intern(consts: &mut Vec<Const>, value: Const) -> u16 {
    if let Some(i) = consts.iter().position(|c| *c == value) {
        return i as u16;
    }
    consts.push(value);
    (consts.len() - 1) as u16
}

/// Builds the bytecode's type section from the checker's types.
///
/// The first five entries are the primitives in tag order, so a primitive is
/// its own index; task types are appended and deduplicated, which is what lets
/// the verifier compare two types by comparing two numbers.
#[derive(Debug)]
struct TypeSection {
    descs: Vec<TypeDesc>,
    index: HashMap<sic_core::TypeId, u32>,
}

impl TypeSection {
    fn new() -> Self {
        Self {
            descs: TypeDesc::PRIMITIVES.to_vec(),
            index: HashMap::new(),
        }
    }

    fn intern(&mut self, ty: sic_core::TypeId, types: &sic_types::Types) -> u32 {
        if let Some(existing) = self.index.get(&ty) {
            return *existing;
        }
        // Trust is erased: the rule it enforces is "this program may not be
        // written", which is a claim about the program rather than about a run,
        // and the bytecode has no use for it.
        if let sic_types::Type::Trust(_, inner) = types.get(ty) {
            let index = self.intern(*inner, types);
            self.index.insert(ty, index);
            return index;
        }

        // A record is reserved before its fields are interned: a type may
        // reach itself through a list, and the reservation is what stops that
        // from recursing forever.
        if let sic_types::Type::Object(object) = types.get(ty) {
            let def = types.object(*object);
            let position = self.descs.len() as u32;
            self.descs.push(TypeDesc::Object {
                name: def.name.clone(),
                fields: Vec::new(),
            });
            self.index.insert(ty, position);
            let declared: Vec<(String, sic_core::TypeId)> = def.fields.clone();
            let fields: Vec<(String, u32)> = declared
                .into_iter()
                .map(|(field_name, t)| {
                    let index = self.intern(t, types);
                    (field_name, index)
                })
                .collect();
            let name = types.object(*object).name.clone();
            self.descs[position as usize] = TypeDesc::Object { name, fields };
            return position;
        }

        let desc = match types.get(ty) {
            sic_types::Type::Bool => TypeDesc::Bool,
            sic_types::Type::Int => TypeDesc::Int,
            sic_types::Type::Float => TypeDesc::Float,
            sic_types::Type::Str => TypeDesc::Str,
            sic_types::Type::Task(inner) => {
                let inner = self.intern(*inner, types);
                TypeDesc::Task(inner)
            }
            sic_types::Type::List(inner) => {
                let inner = self.intern(*inner, types);
                TypeDesc::List(inner)
            }
            // Unit, and anything the checker could not name, is unit here.
            _ => TypeDesc::Unit,
        };
        let position = match desc.primitive_index() {
            Some(i) => i,
            None => match self.descs.iter().position(|d| *d == desc) {
                Some(i) => i as u32,
                None => {
                    self.descs.push(desc);
                    self.descs.len() as u32 - 1
                }
            },
        };
        self.index.insert(ty, position);
        position
    }
}

struct Compiled {
    code: Vec<Inst>,
    reg_count: u8,
    /// `(offset within the function, span)` for the debug section.
    spans: Vec<(u32, Span)>,
    /// `(offset within the function, policy)` for the policy section.
    policies: Vec<(u32, HirCallPolicy)>,
}

struct FnCompile<'a> {
    func: &'a HirFunc,
    /// The module's type section, so an instruction can name a type.
    types: &'a mut TypeSection,
    /// The checker's type table, which the section is built from.
    type_table: &'a sic_types::Types,
    /// The module's constant pool. A function may need to add to it: `-x`
    /// compiles to `0 - x`, and a bare `return` has to load unit. Adding those
    /// on demand keeps a pool free of constants no instruction mentions.
    consts: &'a mut Vec<Const>,
    code: Vec<Inst>,
    spans: Vec<(u32, Span)>,
    /// Where each block starts, once it has been emitted.
    block_starts: HashMap<BlockId, u32>,
    /// Jumps waiting for their target block to be placed.
    fixups: Vec<(u32, BlockId)>,
    /// First register above the locals, used for call arguments and for the
    /// zero operand of a negation.
    scratch_base: usize,
    scratch_used: usize,
    /// Policies collected as capability calls are emitted, keyed by the offset
    /// of the instruction they belong to.
    policies: Vec<(u32, HirCallPolicy)>,
    errors: Vec<CompileError>,
}

impl<'a> FnCompile<'a> {
    fn new(
        func: &'a HirFunc,
        consts: &'a mut Vec<Const>,
        types: &'a mut TypeSection,
        type_table: &'a sic_types::Types,
    ) -> Self {
        Self {
            func,
            types,
            type_table,
            consts,
            code: Vec::new(),
            spans: Vec::new(),
            block_starts: HashMap::new(),
            fixups: Vec::new(),
            scratch_base: func.locals.len(),
            scratch_used: 0,
            policies: Vec::new(),
            errors: Vec::new(),
        }
    }

    fn run(mut self) -> Result<Compiled, Vec<CompileError>> {
        // Blocks are emitted in the order they were created, which makes the
        // fallthrough below match how the lowering built them.
        for (index, block) in self.func.blocks.iter().enumerate() {
            self.block_starts.insert(block.id, self.code.len() as u32);
            for inst in &block.insts {
                self.inst(inst);
            }
            let next = self.func.blocks.get(index + 1).map(|b| b.id);
            self.term(&block.term, next);
        }

        for (at, target) in std::mem::take(&mut self.fixups) {
            let Some(dest) = self.block_starts.get(&target).copied() else {
                self.errors.push(CompileError::new(format!(
                    "bb{} was never emitted",
                    target.0
                )));
                continue;
            };
            // A jump offset counts instructions from the one after the jump.
            let offset = dest as i64 - (at as i64 + 1);
            match i16::try_from(offset) {
                Ok(o) => {
                    let inst = self.code[at as usize];
                    let op = inst.op().expect("emitted by this compiler");
                    self.code[at as usize] = Inst::asbx(op, inst.a(), o);
                }
                Err(_) => self.errors.push(CompileError::new(format!(
                    "jump distance {offset} does not fit in 16 bits in `{}`",
                    self.func.name
                ))),
            }
        }

        let reg_count = self.scratch_base + self.scratch_used;
        if reg_count > MAX_REGISTERS {
            self.errors.push(CompileError::new(format!(
                "`{}` needs {reg_count} registers, the limit is {MAX_REGISTERS}",
                self.func.name
            )));
        }

        if self.errors.is_empty() {
            Ok(Compiled {
                code: self.code,
                reg_count: reg_count as u8,
                spans: self.spans,
                policies: self.policies,
            })
        } else {
            Err(self.errors)
        }
    }

    fn emit(&mut self, inst: Inst, span: Span) {
        let offset = self.code.len() as u32;
        // Only record a position when it differs from the previous one, which
        // keeps the debug section roughly one entry per source expression.
        if self.spans.last().map(|(_, s)| *s) != Some(span) {
            self.spans.push((offset, span));
        }
        self.code.push(inst);
    }

    /// Registers a jump whose target is not placed yet. The offset is patched in
    /// once every block has an address.
    fn emit_jump(&mut self, op: Op, a: u8, target: BlockId, span: Span) {
        let at = self.code.len() as u32;
        self.emit(Inst::asbx(op, a, 0), span);
        self.fixups.push((at, target));
    }

    /// The type section index for a type, as a byte operand.
    fn type_index(&mut self, ty: sic_core::TypeId) -> u8 {
        let index = self.types.intern(ty, self.type_table);
        match u8::try_from(index) {
            Ok(index) => index,
            Err(_) => {
                self.errors.push(CompileError::new(
                    "a module can name at most 256 types in an instruction",
                ));
                0
            }
        }
    }

    fn type_index_u32(&mut self, ty: sic_core::TypeId) -> u32 {
        self.types.intern(ty, self.type_table)
    }

    fn reg(&mut self, local: sic_core::LocalId) -> u8 {
        let index = local.index();
        if index >= MAX_REGISTERS {
            self.errors.push(CompileError::new(format!(
                "`{}` uses more than {MAX_REGISTERS} locals",
                self.func.name
            )));
            return 0;
        }
        index as u8
    }

    /// Reserves `n` consecutive scratch registers and returns the first.
    /// Lays a call's operands out the way every caller has to, and answers
    /// with the window they start at.
    ///
    /// Four instructions share this and none of them chose it: consecutive
    /// registers are the calling convention, so `CALL`, `CALL_CAP`, `SPAWN`
    /// and `MAKE_OBJECT` all have to place their operands the same way.
    /// Written out four times it was a layout four places agreed about by
    /// remembering to, which is what a shared job looks like just before it
    /// stops being shared.
    ///
    /// Evaluation already happened; this only moves.
    fn operands(
        &mut self,
        args: &[sic_core::LocalId],
        dst: sic_core::LocalId,
        span: Span,
    ) -> (u8, u8) {
        let base = self.scratch(args.len());
        for (i, arg) in args.iter().enumerate() {
            let src = self.reg(*arg);
            self.emit(Inst::abc(Op::Move, base + i as u8, src, 0), span);
        }
        (self.reg(dst), base)
    }

    fn scratch(&mut self, n: usize) -> u8 {
        self.scratch_used = self.scratch_used.max(n);
        let base = self.scratch_base;
        if base + n > MAX_REGISTERS {
            self.errors.push(CompileError::new(format!(
                "`{}` needs more than {MAX_REGISTERS} registers",
                self.func.name
            )));
            return 0;
        }
        base as u8
    }

    fn inst(&mut self, inst: &HirInst) {
        let span = inst.span;
        match &inst.kind {
            InstKind::Const { dst, k } => {
                let dst = self.reg(*dst);
                match u16::try_from(k.0) {
                    Ok(idx) => self.emit(Inst::abx(Op::LoadConst, dst, idx), span),
                    Err(_) => self.errors.push(CompileError::new(
                        "a constant index does not fit in 16 bits".to_string(),
                    )),
                }
            }
            InstKind::Move { dst, src } => {
                let (dst, src) = (self.reg(*dst), self.reg(*src));
                if dst != src {
                    self.emit(Inst::abc(Op::Move, dst, src, 0), span);
                }
            }
            InstKind::Un { dst, op, x } => {
                let (dst, x) = (self.reg(*dst), self.reg(*x));
                match op {
                    // There is no NEG opcode: `-x` is `0 - x`, with the zero
                    // materialized in a scratch register.
                    UnOp::Neg => {
                        let zero = self.scratch(1);
                        let zero_idx = intern(self.consts, Const::I64(0));
                        self.emit(Inst::abx(Op::LoadConst, zero, zero_idx), span);
                        self.emit(Inst::abc(Op::SubI64, dst, zero, x), span);
                    }
                    UnOp::Not => self.emit(Inst::abc(Op::Not, dst, x, 0), span),
                }
            }
            InstKind::Bin { dst, op, l, r } => {
                let (dst, l, r) = (self.reg(*dst), self.reg(*l), self.reg(*r));
                let opcode = match op {
                    BinOp::Add => Op::AddI64,
                    BinOp::Sub => Op::SubI64,
                    BinOp::Mul => Op::MulI64,
                    BinOp::Div => Op::DivI64,
                    BinOp::Rem => Op::RemI64,
                    BinOp::Eq => Op::Eq,
                    BinOp::Ne => Op::Ne,
                    BinOp::Lt => Op::Lt,
                    BinOp::Le => Op::Le,
                    BinOp::Gt => Op::Gt,
                    BinOp::Ge => Op::Ge,
                    BinOp::And | BinOp::Or => {
                        // Short-circuiting operators became branches during
                        // lowering, so one here would be a bug in sic-ir.
                        self.errors.push(CompileError::new(
                            "`&&` and `||` must be lowered to branches".to_string(),
                        ));
                        return;
                    }
                };
                self.emit(Inst::abc(opcode, dst, l, r), span);
            }
            InstKind::Call { dst, func, args } => {
                let (dst, base) = self.operands(args, *dst, span);
                match u8::try_from(func.0) {
                    Ok(f) => self.emit(Inst::abc(Op::Call, dst, f, base), span),
                    Err(_) => self
                        .errors
                        .push(CompileError::new("a function index does not fit in a byte")),
                }
            }
            InstKind::CallCap {
                dst,
                cap,
                args,
                policy,
            } => {
                let (dst, base) = self.operands(args, *dst, span);
                match u8::try_from(cap.0) {
                    Ok(c) => {
                        // The policy is keyed by the instruction's offset: a
                        // four-byte instruction has no room for it, and a side
                        // table can be read without executing anything.
                        if !policy.is_empty() {
                            self.policies.push((self.code.len() as u32, policy.clone()));
                        }
                        self.emit(Inst::abc(Op::CallCap, dst, c, base), span);
                    }
                    Err(_) => self.errors.push(CompileError::new(
                        "a capability index does not fit in a byte",
                    )),
                }
            }
            InstKind::Spawn { dst, func, args } => {
                let (dst, base) = self.operands(args, *dst, span);
                match u8::try_from(func.0) {
                    Ok(f) => self.emit(Inst::abc(Op::Spawn, dst, f, base), span),
                    Err(_) => self
                        .errors
                        .push(CompileError::new("a function index does not fit in a byte")),
                }
            }
            InstKind::Await { dst, task } => {
                let (dst, task) = (self.reg(*dst), self.reg(*task));
                self.emit(Inst::abc(Op::Await, dst, task, 0), span);
            }
            InstKind::MakeObject { dst, ty, fields } => {
                let (dst, base) = self.operands(fields, *dst, span);
                let type_index = self.type_index(*ty);
                self.emit(Inst::abc(Op::MakeObject, dst, type_index, base), span);
            }
            InstKind::GetField { dst, base, index } => {
                let (dst, base) = (self.reg(*dst), self.reg(*base));
                match u8::try_from(*index) {
                    Ok(field) => self.emit(Inst::abc(Op::GetField, dst, base, field), span),
                    Err(_) => self
                        .errors
                        .push(CompileError::new("a record can have at most 256 fields")),
                }
            }
            InstKind::MakeList { dst, ty, elements } => {
                let dst_reg = self.reg(*dst);
                if elements.is_empty() {
                    // An empty list has no elements to take a type from, so it
                    // is a constant carrying its own.
                    let type_index = self.type_index_u32(*ty);
                    let konst = intern(self.consts, Const::EmptyList(type_index));
                    self.emit(Inst::abx(Op::LoadConst, dst_reg, konst), span);
                    return;
                }
                let base = self.scratch(elements.len());
                for (i, element) in elements.iter().enumerate() {
                    let src = self.reg(*element);
                    self.emit(Inst::abc(Op::Move, base + i as u8, src, 0), span);
                }
                match u8::try_from(elements.len()) {
                    Ok(count) => self.emit(Inst::abc(Op::MakeList, dst_reg, base, count), span),
                    Err(_) => self.errors.push(CompileError::new(
                        "a list literal can have at most 255 elements",
                    )),
                }
            }
            InstKind::GetIndex { dst, base, index } => {
                let (dst, base, index) = (self.reg(*dst), self.reg(*base), self.reg(*index));
                self.emit(Inst::abc(Op::GetIndex, dst, base, index), span);
            }
            InstKind::Len { dst, src } => {
                let (dst, src) = (self.reg(*dst), self.reg(*src));
                self.emit(Inst::abc(Op::Len, dst, src, 0), span);
            }
            InstKind::FromJson { dst, ty, src } => {
                let (dst, src) = (self.reg(*dst), self.reg(*src));
                let type_index = self.type_index(*ty);
                self.emit(Inst::abc(Op::FromJson, dst, type_index, src), span);
            }
            InstKind::Log { level, msg } => {
                let msg = self.reg(*msg);
                self.emit(Inst::abc(Op::Log, level_code(*level), msg, 0), span);
            }
        }
    }

    fn term(&mut self, term: &Terminator, next: Option<BlockId>) {
        let span = term.span;
        match &term.kind {
            Term::Jump(target) => {
                // Falling through costs nothing when the target is next.
                if next != Some(*target) {
                    self.emit_jump(Op::Jump, 0, *target, span);
                }
            }
            Term::Branch {
                cond,
                then_bb,
                else_bb,
            } => {
                let cond = self.reg(*cond);
                if next == Some(*then_bb) {
                    self.emit_jump(Op::JumpIfNot, cond, *else_bb, span);
                } else if next == Some(*else_bb) {
                    self.emit_jump(Op::JumpIf, cond, *then_bb, span);
                } else {
                    self.emit_jump(Op::JumpIf, cond, *then_bb, span);
                    self.emit_jump(Op::Jump, 0, *else_bb, span);
                }
            }
            Term::Return(Some(value)) => {
                let r = self.reg(*value);
                self.emit(Inst::abc(Op::Return, r, 0, 0), span);
            }
            Term::Return(None) => {
                // Returning nothing still returns a value: unit.
                let r = self.scratch(1);
                let unit_idx = intern(self.consts, Const::Unit);
                self.emit(Inst::abx(Op::LoadConst, r, unit_idx), span);
                self.emit(Inst::abc(Op::Return, r, 0, 0), span);
            }
            Term::Fail(value) => {
                let r = self.reg(*value);
                self.emit(Inst::abc(Op::Fail, r, 0, 0), span);
            }
        }
    }
}

#[cfg(test)]
mod tests;
