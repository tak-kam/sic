//! Lowering from the typed AST to HIR.
//!
//! Runs only after type checking reported no errors, so it can assume every
//! expression has a type and every name resolves. Anything it cannot assume
//! would be a bug in the checker, and is reached with `unreachable!`.

use std::collections::HashMap;

use sic_core::{BlockId, ConstIdx, LocalId, Span, TypeId};
use sic_syntax::ast::*;
use sic_types::{Res, Typed};

use crate::hir::*;

/// Lowers a checked module.
pub fn lower(module: &Module, typed: &Typed) -> Hir {
    let mut consts = ConstPool::default();
    let funcs = typed
        .fns
        .iter()
        .map(|info| {
            let Item::Fn(decl) = &module.items[info.item_index];
            FnLower::new(typed, &mut consts, info.local_types.clone(), info.ret).run(decl)
        })
        .collect();
    Hir {
        funcs,
        consts: consts.values,
    }
}

/// Deduplicating constant pool.
#[derive(Default)]
struct ConstPool {
    values: Vec<Const>,
    index: HashMap<ConstKey, ConstIdx>,
}

/// A hashable key for a constant. `f64` is keyed by its bits, so `0.0` and
/// `-0.0` stay distinct entries rather than silently merging.
#[derive(PartialEq, Eq, Hash)]
enum ConstKey {
    Unit,
    Bool(bool),
    I64(i64),
    F64(u64),
    Str(String),
}

impl ConstPool {
    fn add(&mut self, value: Const) -> ConstIdx {
        let key = match &value {
            Const::Unit => ConstKey::Unit,
            Const::Bool(b) => ConstKey::Bool(*b),
            Const::I64(v) => ConstKey::I64(*v),
            Const::F64(v) => ConstKey::F64(v.to_bits()),
            Const::Str(s) => ConstKey::Str(s.clone()),
        };
        if let Some(idx) = self.index.get(&key) {
            return *idx;
        }
        let idx = ConstIdx(self.values.len() as u32);
        self.values.push(value);
        self.index.insert(key, idx);
        idx
    }
}

struct FnLower<'a> {
    typed: &'a Typed,
    consts: &'a mut ConstPool,
    locals: Vec<TypeId>,
    ret: TypeId,
    blocks: Vec<HirBlock>,
    /// The block instructions are currently appended to.
    cur: BlockId,
    /// Set once the current block has a terminator; the next instruction opens
    /// a fresh (unreachable) block instead of writing past it.
    sealed: bool,
}

impl<'a> FnLower<'a> {
    fn new(typed: &'a Typed, consts: &'a mut ConstPool, locals: Vec<TypeId>, ret: TypeId) -> Self {
        Self {
            typed,
            consts,
            locals,
            ret,
            blocks: Vec::new(),
            cur: BlockId(0),
            sealed: false,
        }
    }

    fn run(mut self, decl: &FnDecl) -> HirFunc {
        let entry = self.new_block();
        self.cur = entry;
        self.block(&decl.body);
        // Falling off the end returns unit.
        if !self.sealed {
            self.terminate(Term::Return(None), decl.body.span);
        }
        HirFunc {
            name: decl.name.name.clone(),
            params: (0..decl.params.len() as u32).map(LocalId).collect(),
            ret: self.ret,
            locals: self.locals,
            blocks: self.blocks,
            entry,
        }
    }

    // ---- block and instruction plumbing ----

    fn new_block(&mut self) -> BlockId {
        let id = BlockId(self.blocks.len() as u32);
        self.blocks.push(HirBlock {
            id,
            insts: Vec::new(),
            // Replaced by `terminate`; a block that is never terminated returns.
            term: Terminator {
                kind: Term::Return(None),
                span: Span::empty(0),
            },
        });
        id
    }

    fn switch_to(&mut self, block: BlockId) {
        self.cur = block;
        self.sealed = false;
    }

    fn emit(&mut self, kind: InstKind, span: Span) {
        if self.sealed {
            // Code after a `return`: keep lowering it into a block nothing jumps
            // to, so the verifier can report it as unreachable.
            let dead = self.new_block();
            self.switch_to(dead);
        }
        self.blocks[self.cur.index()]
            .insts
            .push(Inst { kind, span });
    }

    fn terminate(&mut self, kind: Term, span: Span) {
        if self.sealed {
            return;
        }
        self.blocks[self.cur.index()].term = Terminator { kind, span };
        self.sealed = true;
    }

    fn temp(&mut self, ty: TypeId) -> LocalId {
        let id = LocalId(self.locals.len() as u32);
        self.locals.push(ty);
        id
    }

    fn constant(&mut self, value: Const, ty: TypeId, span: Span) -> LocalId {
        let k = self.consts.add(value);
        let dst = self.temp(ty);
        self.emit(InstKind::Const { dst, k }, span);
        dst
    }

    // ---- statements ----

    fn block(&mut self, block: &Block) {
        for stmt in &block.stmts {
            self.stmt(stmt);
        }
    }

    fn stmt(&mut self, stmt: &Stmt) {
        match stmt {
            Stmt::Let { id, init, span, .. } => {
                let value = self.expr(init);
                let Some(Res::Local(slot)) = self.typed.res_of(*id) else {
                    unreachable!("a `let` must resolve to a local");
                };
                self.emit(
                    InstKind::Move {
                        dst: slot,
                        src: value,
                    },
                    *span,
                );
            }
            Stmt::Return { value, span, .. } => {
                let v = value.as_ref().map(|e| self.expr(e));
                self.terminate(Term::Return(v), *span);
            }
            Stmt::If(if_stmt) => self.if_stmt(if_stmt),
            Stmt::Expr { expr, .. } => {
                self.expr(expr);
            }
        }
    }

    fn if_stmt(&mut self, if_stmt: &IfStmt) {
        let cond = self.expr(&if_stmt.cond);
        let then_bb = self.new_block();
        let else_bb = self.new_block();
        let join = self.new_block();
        self.terminate(
            Term::Branch {
                cond,
                then_bb,
                else_bb,
            },
            if_stmt.cond.span,
        );

        self.switch_to(then_bb);
        self.block(&if_stmt.then_block);
        self.terminate(Term::Jump(join), if_stmt.then_block.span);

        self.switch_to(else_bb);
        match if_stmt.else_branch.as_deref() {
            Some(ElseBranch::Block(b)) => self.block(b),
            Some(ElseBranch::If(inner)) => self.if_stmt(inner),
            None => {}
        }
        self.terminate(Term::Jump(join), if_stmt.span);

        self.switch_to(join);
    }

    // ---- expressions ----

    fn expr(&mut self, e: &Expr) -> LocalId {
        let ty = self.typed.type_of(e.id);
        match &e.kind {
            ExprKind::Int(v) => self.constant(Const::I64(*v), ty, e.span),
            ExprKind::Float(v) => self.constant(Const::F64(*v), ty, e.span),
            ExprKind::Bool(v) => self.constant(Const::Bool(*v), ty, e.span),
            ExprKind::Str(s) => self.constant(Const::Str(s.clone()), ty, e.span),
            ExprKind::Path(_) => match self.typed.res_of(e.id) {
                // Reading a variable needs no instruction: the local is the value.
                Some(Res::Local(slot)) => slot,
                _ => unreachable!("a path expression must resolve to a local"),
            },
            ExprKind::Unary { op, operand } => {
                let x = self.expr(operand);
                let dst = self.temp(ty);
                self.emit(InstKind::Un { dst, op: *op, x }, e.span);
                dst
            }
            ExprKind::Binary { op, lhs, rhs } if matches!(op, BinOp::And | BinOp::Or) => {
                self.short_circuit(*op, lhs, rhs, ty, e.span)
            }
            ExprKind::Binary { op, lhs, rhs } => {
                let l = self.expr(lhs);
                let r = self.expr(rhs);
                let dst = self.temp(ty);
                self.emit(InstKind::Bin { dst, op: *op, l, r }, e.span);
                dst
            }
            ExprKind::Call { callee, args } => {
                let Some(Res::Fn(func)) = self.typed.res_of(callee.id) else {
                    unreachable!("a call must resolve to a function");
                };
                // Arguments are evaluated into locals first; the bytecode
                // compiler is what moves them into consecutive registers.
                let args: Vec<LocalId> = args.iter().map(|a| self.expr(a)).collect();
                let dst = self.temp(ty);
                self.emit(InstKind::Call { dst, func, args }, e.span);
                dst
            }
            ExprKind::Null | ExprKind::Field { .. } | ExprKind::Error => {
                unreachable!("rejected by the type checker")
            }
        }
    }

    /// `&&` and `||` evaluate the right-hand side only when it can change the
    /// result, so they lower to a branch rather than to an instruction.
    fn short_circuit(
        &mut self,
        op: BinOp,
        lhs: &Expr,
        rhs: &Expr,
        ty: TypeId,
        span: Span,
    ) -> LocalId {
        let res = self.temp(ty);
        let l = self.expr(lhs);
        // Writing the result before branching means it is initialized on both
        // paths, which is what the verifier's merge rule requires.
        self.emit(InstKind::Move { dst: res, src: l }, span);

        let rhs_bb = self.new_block();
        let join = self.new_block();
        let (then_bb, else_bb) = match op {
            BinOp::And => (rhs_bb, join), // false short-circuits
            BinOp::Or => (join, rhs_bb),  // true short-circuits
            _ => unreachable!("not a short-circuiting operator"),
        };
        self.terminate(
            Term::Branch {
                cond: l,
                then_bb,
                else_bb,
            },
            span,
        );

        self.switch_to(rhs_bb);
        let r = self.expr(rhs);
        self.emit(InstKind::Move { dst: res, src: r }, span);
        self.terminate(Term::Jump(join), span);

        self.switch_to(join);
        res
    }
}

/// Convenience for the CLI and tests: parse, check, and lower in one call.
pub fn compile_to_hir(src: &str) -> Result<Hir, Vec<sic_core::Diagnostic>> {
    let (module, mut diags) = sic_syntax::parse(src);
    let (typed, type_diags) = sic_types::check(&module);
    diags.extend(type_diags);
    if diags.iter().any(|d| d.is_error()) {
        return Err(diags);
    }
    Ok(lower(&module, &typed))
}
