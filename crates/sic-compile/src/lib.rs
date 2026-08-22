//! Lowering from HIR to bytecode.
//!
//! Register allocation is deliberately trivial: local `n` becomes register `n`,
//! and a small scratch area above the locals holds call arguments. Anything
//! smarter would have to be justified by a measurement, and there is none yet.

use std::collections::HashMap;

use sic_bytecode::inst::{Inst, Op};
use sic_bytecode::program::*;
use sic_core::{BlockId, SourceFile, Span};
use sic_ir::hir::{
    BinOp, Const as HirConst, Hir, HirFunc, Inst as HirInst, InstKind, Term, Terminator, UnOp,
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

pub fn compile(hir: &Hir, file: &SourceFile) -> Result<Program, Vec<CompileError>> {
    let mut errors = Vec::new();

    if hir.funcs.len() > MAX_FUNCS {
        errors.push(CompileError::new(format!(
            "a module can hold at most {MAX_FUNCS} functions, found {}",
            hir.funcs.len()
        )));
    }

    let mut consts: Vec<Const> = hir.consts.iter().map(to_bytecode_const).collect();

    let mut program = Program {
        // The type section lists the tags in tag order, so a tag doubles as its
        // own index.
        types: vec![
            TypeTag::Unit,
            TypeTag::Bool,
            TypeTag::Int,
            TypeTag::Float,
            TypeTag::Str,
        ],
        debug: DebugInfo {
            source_name: file.name().to_string(),
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
            params: c.params.iter().map(|t| type_tag_of(*t) as u32).collect(),
            ret_type: type_tag_of(c.ret) as u32,
        })
        .collect();

    for func in &hir.funcs {
        match FnCompile::new(func, &mut consts).run() {
            Ok(compiled) => {
                let code_off = program.code.len() as u32;
                for (offset, span) in compiled.spans {
                    let pos = file.line_col(span.lo);
                    program
                        .debug
                        .lines
                        .push((code_off + offset, pos.line, pos.col));
                }
                program.code.extend(compiled.code);
                program.funcs.push(FuncDef {
                    name: func.name.clone(),
                    params: func
                        .params
                        .iter()
                        .map(|p| type_tag_of(func.locals[p.index()]) as u32)
                        .collect(),
                    reg_count: compiled.reg_count,
                    ret_type: type_tag_of(func.ret) as u32,
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

    if errors.is_empty() {
        Ok(program)
    } else {
        Err(errors)
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

/// Maps a checked type onto the coarser tag the bytecode level uses.
fn type_tag_of(ty: sic_core::TypeId) -> TypeTag {
    use sic_types::Types;
    match ty {
        Types::BOOL => TypeTag::Bool,
        Types::INT => TypeTag::Int,
        Types::FLOAT => TypeTag::Float,
        Types::STR => TypeTag::Str,
        // Unit, and anything the checker could not name, is represented as unit.
        _ => TypeTag::Unit,
    }
}

struct Compiled {
    code: Vec<Inst>,
    reg_count: u8,
    /// `(offset within the function, span)` for the debug section.
    spans: Vec<(u32, Span)>,
}

struct FnCompile<'a> {
    func: &'a HirFunc,
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
    errors: Vec<CompileError>,
}

impl<'a> FnCompile<'a> {
    fn new(func: &'a HirFunc, consts: &'a mut Vec<Const>) -> Self {
        Self {
            func,
            consts,
            code: Vec::new(),
            spans: Vec::new(),
            block_starts: HashMap::new(),
            fixups: Vec::new(),
            scratch_base: func.locals.len(),
            scratch_used: 0,
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
                let base = self.scratch(args.len());
                // Arguments are copied into consecutive registers because that
                // is the calling convention; evaluation already happened.
                for (i, arg) in args.iter().enumerate() {
                    let src = self.reg(*arg);
                    self.emit(Inst::abc(Op::Move, base + i as u8, src, 0), span);
                }
                let dst = self.reg(*dst);
                match u8::try_from(func.0) {
                    Ok(f) => self.emit(Inst::abc(Op::Call, dst, f, base), span),
                    Err(_) => self
                        .errors
                        .push(CompileError::new("a function index does not fit in a byte")),
                }
            }
            InstKind::CallCap { dst, cap, args, .. } => {
                // Same calling convention as CALL: the arguments go into
                // consecutive scratch registers.
                let base = self.scratch(args.len());
                for (i, arg) in args.iter().enumerate() {
                    let src = self.reg(*arg);
                    self.emit(Inst::abc(Op::Move, base + i as u8, src, 0), span);
                }
                let dst = self.reg(*dst);
                match u8::try_from(cap.0) {
                    Ok(c) => self.emit(Inst::abc(Op::CallCap, dst, c, base), span),
                    Err(_) => self.errors.push(CompileError::new(
                        "a capability index does not fit in a byte",
                    )),
                }
            }
            InstKind::Spawn { .. } | InstKind::Await { .. } | InstKind::Log { .. } => {
                self.errors.push(CompileError::new(
                    "tasks and logging arrive in a later phase".to_string(),
                ));
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
