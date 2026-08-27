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
            let Item::Fn(decl) = &module.items[info.item_index] else {
                unreachable!("a function's item index must name a function");
            };
            FnLower::new(typed, &mut consts, info.local_types.clone(), info.ret).run(decl)
        })
        .collect();
    Hir {
        funcs,
        consts: consts.values,
        caps: typed.caps.clone(),
        types: typed.types.clone(),
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
            Stmt::Log {
                level,
                message,
                span,
                ..
            } => {
                let msg = self.expr(message);
                self.emit(InstKind::Log { level: *level, msg }, *span);
            }
            Stmt::If(if_stmt) => self.if_stmt(if_stmt),
            Stmt::For(for_stmt) => self.for_stmt(for_stmt),
            Stmt::Expr { expr, .. } => {
                self.expr(expr);
            }
        }
    }

    /// `for x in xs { ... }`, as an index and a backward jump.
    ///
    /// No instruction was added for this. The list is evaluated once, its
    /// length once, and what is left is a counter, a comparison, `GET_INDEX`
    /// and the jump the bytecode has always been able to encode. The counter is
    /// a temporary the source cannot name, so nothing can move it and the loop
    /// runs exactly `len(xs)` times.
    ///
    /// ```text
    ///   list = <iter>            ; once
    ///   n    = LEN list          ; once
    ///   i    = 0
    /// head: cond = i < n
    ///   BRANCH cond -> body, exit
    /// body: x = GET_INDEX list i
    ///   <body>
    ///   i = i + 1
    ///   JUMP head                ; the backward edge
    /// exit:
    /// ```
    fn for_stmt(&mut self, for_stmt: &ForStmt) {
        let list = self.expr(&for_stmt.iter);
        let count = self.temp(sic_types::Types::INT);
        self.emit(
            InstKind::Len {
                dst: count,
                src: list,
            },
            for_stmt.iter.span,
        );
        let index = self.constant(Const::I64(0), sic_types::Types::INT, for_stmt.span);

        let head = self.new_block();
        let body = self.new_block();
        let exit = self.new_block();
        self.terminate(Term::Jump(head), for_stmt.span);

        self.switch_to(head);
        let more = self.temp(sic_types::Types::BOOL);
        self.emit(
            InstKind::Bin {
                dst: more,
                op: BinOp::Lt,
                l: index,
                r: count,
            },
            for_stmt.span,
        );
        self.terminate(
            Term::Branch {
                cond: more,
                then_bb: body,
                else_bb: exit,
            },
            for_stmt.span,
        );

        self.switch_to(body);
        let Some(Res::Local(slot)) = self.typed.res_of(for_stmt.id) else {
            unreachable!("a `for` binding must resolve to a local");
        };
        self.emit(
            InstKind::GetIndex {
                dst: slot,
                base: list,
                index,
            },
            for_stmt.var.span,
        );
        self.block(&for_stmt.body);
        // A `return` in the body leaves no path back to the head, and stepping
        // the counter into a block nothing reaches would be code the verifier
        // then has to report as unreachable.
        if !self.sealed {
            let one = self.constant(Const::I64(1), sic_types::Types::INT, for_stmt.span);
            self.emit(
                InstKind::Bin {
                    dst: index,
                    op: BinOp::Add,
                    l: index,
                    r: one,
                },
                for_stmt.span,
            );
            self.terminate(Term::Jump(head), for_stmt.body.span);
        }

        self.switch_to(exit);
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
            ExprKind::Call {
                callee,
                args,
                policy,
            } => {
                // Arguments are evaluated into locals first; the bytecode
                // compiler is what moves them into consecutive registers.
                let args: Vec<LocalId> = args.iter().map(|a| self.expr(a)).collect();
                let dst = self.temp(ty);
                match self.typed.res_of(callee.id) {
                    Some(Res::Fn(func)) => self.emit(InstKind::Call { dst, func, args }, e.span),
                    // A built-in becomes an instruction rather than a call.
                    Some(Res::Builtin(sic_types::Builtin::Len)) => {
                        self.emit(InstKind::Len { dst, src: args[0] }, e.span)
                    }
                    Some(Res::Builtin(sic_types::Builtin::Contains)) => self.emit(
                        InstKind::Contains {
                            dst,
                            s: args[0],
                            sub: args[1],
                        },
                        e.span,
                    ),
                    Some(Res::Builtin(sic_types::Builtin::StartsWith)) => self.emit(
                        InstKind::StartsWith {
                            dst,
                            s: args[0],
                            prefix: args[1],
                        },
                        e.span,
                    ),
                    Some(Res::Builtin(sic_types::Builtin::Approve)) => {
                        self.approve(dst, args[0], args[1], e.span)
                    }
                    Some(Res::Builtin(sic_types::Builtin::Choose)) => {
                        self.choose(dst, args[0], args[1], e.span)
                    }
                    // An agent is a model call and a validation. Nothing below
                    // this point knows what an agent is.
                    Some(Res::Agent(agent)) => {
                        let info = self.typed.agents[agent.index()].clone();
                        let raw = self.temp(sic_types::Types::STR);
                        // The declaration is the only place the shape of the
                        // answer is written down, so it travels with the
                        // prompt. Without it whoever answers has been asked a
                        // question and not told what an answer looks like.
                        let shape = self.typed.types.shape(info.output);
                        let mut args = args;
                        args.push(self.constant(Const::Str(shape), sic_types::Types::STR, e.span));
                        self.emit(
                            InstKind::CallCap {
                                dst: raw,
                                cap: info.cap,
                                args,
                                policy: crate::hir::CallPolicy {
                                    budget: info.budget,
                                    conversation: info.conversation,
                                    tools: info.tools,
                                    deadline_ms: info.deadline_ms,
                                    ..Default::default()
                                },
                            },
                            e.span,
                        );
                        self.emit(
                            InstKind::FromJson {
                                dst,
                                ty: info.output,
                                src: raw,
                            },
                            e.span,
                        );
                    }
                    Some(Res::Builtin(sic_types::Builtin::FromJson)) => self.emit(
                        InstKind::FromJson {
                            dst,
                            ty,
                            src: args[0],
                        },
                        e.span,
                    ),
                    Some(Res::Cap(cap)) => {
                        // An omitted argument vector is an empty one. Filling
                        // it in here means everything downstream - the
                        // verifier, the VM, the broker - sees calls of one
                        // shape. See `docs/design/arguments.md`.
                        let mut args = args;
                        let entry = &self.typed.caps[cap.index()];
                        if args.len() + 1 == entry.params.len() {
                            let ty = entry.params[args.len()];
                            let empty = match ty == sic_types::Types::STR {
                                true => self.constant(Const::Str(String::new()), ty, e.span),
                                false => {
                                    let empty = self.temp(ty);
                                    self.emit(
                                        InstKind::MakeList {
                                            dst: empty,
                                            ty,
                                            elements: Vec::new(),
                                        },
                                        e.span,
                                    );
                                    empty
                                }
                            };
                            args.push(empty);
                        }
                        self.emit(
                            InstKind::CallCap {
                                dst,
                                cap,
                                args,
                                policy: crate::hir::CallPolicy {
                                    attempts: policy.attempts,
                                    timeout_ms: policy.timeout_ms,
                                    ..Default::default()
                                },
                            },
                            e.span,
                        )
                    }
                    _ => unreachable!("a call must resolve to a function or a capability"),
                }
                dst
            }
            ExprKind::Spawn { callee, args } => {
                let Some(Res::Fn(func)) = self.typed.res_of(callee.id) else {
                    unreachable!("a spawn must resolve to a function");
                };
                let args: Vec<LocalId> = args.iter().map(|a| self.expr(a)).collect();
                let dst = self.temp(ty);
                self.emit(InstKind::Spawn { dst, func, args }, e.span);
                dst
            }
            ExprKind::Await { task } => {
                let task = self.expr(task);
                let dst = self.temp(ty);
                self.emit(InstKind::Await { dst, task }, e.span);
                dst
            }
            ExprKind::Struct { fields, .. } => {
                // Fields are stored in declaration order, which is not
                // necessarily the order they were written in.
                let Some(object) = self.typed.types.as_object(ty) else {
                    unreachable!("a struct literal has a record type");
                };
                let declared = self.typed.types.object(object).fields.clone();
                let mut slots: Vec<Option<LocalId>> = vec![None; declared.len()];
                for field in fields {
                    let value = self.expr(&field.value);
                    if let Some(position) = declared.iter().position(|(n, _)| *n == field.name.name)
                    {
                        slots[position] = Some(value);
                    }
                }
                let values: Vec<LocalId> = slots
                    .into_iter()
                    .map(|slot| slot.expect("the checker required every field"))
                    .collect();
                let dst = self.temp(ty);
                self.emit(
                    InstKind::MakeObject {
                        dst,
                        ty,
                        fields: values,
                    },
                    e.span,
                );
                dst
            }
            ExprKind::List { elements } => {
                let values: Vec<LocalId> = elements.iter().map(|el| self.expr(el)).collect();
                let dst = self.temp(ty);
                self.emit(
                    InstKind::MakeList {
                        dst,
                        ty,
                        elements: values,
                    },
                    e.span,
                );
                dst
            }
            ExprKind::Index { base, index } => {
                let base = self.expr(base);
                let index = self.expr(index);
                let dst = self.temp(ty);
                self.emit(InstKind::GetIndex { dst, base, index }, e.span);
                dst
            }
            ExprKind::Field { base, name } => {
                // Trust is erased here: the layout of a record does not depend
                // on where the value came from.
                let base_ty = self.typed.types.untrusted(self.typed.type_of(base.id));
                let base = self.expr(base);
                let Some(object) = self.typed.types.as_object(base_ty) else {
                    unreachable!("field access needs a record type");
                };
                let (index, _) = self
                    .typed
                    .types
                    .object(object)
                    .field(&name.name)
                    .expect("the checker resolved this field");
                let dst = self.temp(ty);
                self.emit(
                    InstKind::GetField {
                        dst,
                        base,
                        index: index as u32,
                    },
                    e.span,
                );
                dst
            }
            ExprKind::Null | ExprKind::Error => {
                unreachable!("rejected by the type checker")
            }
        }
    }

    /// `choose(question, options)`: ask a person which one, and read that one
    /// out of the list.
    ///
    /// The capability answers with an index, so the value handed back comes
    /// from the list this call already built. `GET_INDEX` refuses an index
    /// outside it, which is the whole of what an answer can get wrong.
    fn choose(&mut self, dst: LocalId, question: LocalId, options: LocalId, span: Span) {
        let Some(cap) = self
            .typed
            .caps
            .iter()
            .position(|c| c.name == "human.choose")
        else {
            unreachable!("the checker required the grant");
        };
        let picked = self.temp(sic_types::Types::INT);
        self.emit(
            InstKind::CallCap {
                dst: picked,
                cap: sic_core::CapId(cap as u32),
                args: vec![question, options],
                policy: crate::hir::CallPolicy::default(),
            },
            span,
        );
        self.emit(
            InstKind::GetIndex {
                dst,
                base: options,
                index: picked,
            },
            span,
        );
    }

    /// `approve(question, value)`: ask a person, and fail the run if the answer
    /// is no.
    ///
    /// The value itself is untouched - trust is a compile-time distinction, and
    /// the same bytes come out - so what this lowers to is the question, the
    /// branch, and the failure.
    fn approve(&mut self, dst: LocalId, question: LocalId, value: LocalId, span: Span) {
        let Some(cap) = self
            .typed
            .caps
            .iter()
            .position(|c| c.name == "human.approve")
        else {
            unreachable!("the checker required the grant");
        };
        let answered = self.temp(sic_types::Types::BOOL);
        self.emit(
            InstKind::CallCap {
                dst: answered,
                cap: sic_core::CapId(cap as u32),
                args: vec![question],
                policy: crate::hir::CallPolicy::default(),
            },
            span,
        );
        self.emit(InstKind::Move { dst, src: value }, span);

        let refused = self.new_block();
        let join = self.new_block();
        self.terminate(
            Term::Branch {
                cond: answered,
                then_bb: join,
                else_bb: refused,
            },
            span,
        );

        self.switch_to(refused);
        let message = self.constant(
            Const::Str("the approval was refused".into()),
            sic_types::Types::STR,
            span,
        );
        self.terminate(Term::Fail(message), span);

        self.switch_to(join);
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
