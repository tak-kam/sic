//! Type checking and name resolution.
//!
//! Functions are checked in declaration order. A function without a return type
//! annotation gets its type from the `return` statements in its body, so calling
//! it before it has been checked requires an annotation (E0306). That rule keeps
//! inference to a single forward pass with no constraint solving.

use std::collections::HashMap;

use sic_core::{CapId, Diagnostic, FuncId, Label, LocalId, NodeId, Span, TypeId};
use sic_syntax::ast::*;

use crate::cap::{self, CapEntry};
use crate::ty::{Type, Types};

/// What a name in an expression refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Res {
    Local(LocalId),
    Fn(FuncId),
    /// A capability, resolved to its index in the module's manifest.
    Cap(CapId),
}

#[derive(Debug, Clone)]
pub struct FnInfo {
    pub name: String,
    /// Parameter types, in order. Parameters occupy locals 0..params.len().
    pub params: Vec<TypeId>,
    pub ret: TypeId,
    /// Types of the named locals, indexed by `LocalId`.
    pub local_types: Vec<TypeId>,
    /// Index of this function in `Module::items`.
    pub item_index: usize,
    pub span: Span,
}

/// Everything the later phases need on top of the AST.
#[derive(Debug)]
pub struct Typed {
    pub types: Types,
    pub fns: Vec<FnInfo>,
    /// The type of every expression, keyed by its `NodeId`.
    pub node_types: HashMap<NodeId, TypeId>,
    /// What each name expression resolved to.
    pub res: HashMap<NodeId, Res>,
    /// The capabilities the module granted itself, in manifest order.
    pub caps: Vec<CapEntry>,
    /// The `main` function, if the module has one.
    pub entry: Option<FuncId>,
}

impl Typed {
    pub fn type_of(&self, node: NodeId) -> TypeId {
        self.node_types.get(&node).copied().unwrap_or(Types::ERROR)
    }

    pub fn res_of(&self, node: NodeId) -> Option<Res> {
        self.res.get(&node).copied()
    }
}

/// Checks a module. Diagnostics are collected rather than raised.
pub fn check(module: &Module) -> (Typed, Vec<Diagnostic>) {
    let mut c = Checker::new();
    c.collect_capabilities(module);
    c.collect_signatures(module);
    c.check_bodies(module);
    c.finish()
}

/// A function signature while checking is still in progress.
struct FnState {
    name: String,
    params: Vec<TypeId>,
    /// `None` means "not annotated and not checked yet", which is what makes a
    /// forward reference an error instead of a guess.
    ret: Option<TypeId>,
    local_types: Vec<TypeId>,
    item_index: usize,
    span: Span,
}

struct Checker {
    types: Types,
    diags: Vec<Diagnostic>,
    fns: Vec<FnState>,
    fn_ids: HashMap<String, FuncId>,
    node_types: HashMap<NodeId, TypeId>,
    res: HashMap<NodeId, Res>,
    caps: Vec<CapEntry>,
    cap_ids: HashMap<String, CapId>,

    // State for the function currently being checked.
    scopes: Vec<Vec<(String, LocalId)>>,
    locals: Vec<TypeId>,
    ret_ty: Option<TypeId>,
    ret_annotated: bool,
}

impl Checker {
    fn new() -> Self {
        Self {
            types: Types::new(),
            diags: Vec::new(),
            fns: Vec::new(),
            fn_ids: HashMap::new(),
            node_types: HashMap::new(),
            res: HashMap::new(),
            caps: Vec::new(),
            cap_ids: HashMap::new(),
            scopes: Vec::new(),
            locals: Vec::new(),
            ret_ty: None,
            ret_annotated: false,
        }
    }

    fn error(
        &mut self,
        code: &'static str,
        msg: impl Into<String>,
        span: Span,
        label: impl Into<String>,
    ) {
        self.diags
            .push(Diagnostic::error(code, msg, Label::new(span, label)));
    }

    fn note(&mut self, note: impl Into<String>) {
        if let Some(last) = self.diags.last_mut() {
            last.notes.push(note.into());
        }
    }

    // ---- pass 0: capability grants ----

    /// Builds the module's manifest from its `allow` blocks.
    ///
    /// This runs before anything else, so that a call can be checked against
    /// the manifest no matter where the grant appears in the file.
    fn collect_capabilities(&mut self, module: &Module) {
        for item in &module.items {
            let Item::Allow(decl) = item else {
                continue;
            };
            for grant in &decl.grants {
                let full = grant.path.full_name();
                let Some(sig) = cap::builtin(&full) else {
                    self.error(
                        "E0321",
                        format!("unknown capability `{full}`"),
                        grant.path.span,
                        "no such capability",
                    );
                    self.note(format!("v0.1 has {}", cap::all_names().join(", ")));
                    continue;
                };
                if self.cap_ids.contains_key(&full) {
                    self.error(
                        "E0323",
                        format!("`{full}` is granted more than once"),
                        grant.span,
                        "already granted",
                    );
                    self.note("a capability has one grant, so its manifest entry is unambiguous");
                    continue;
                }
                let constraint = match &grant.constraint {
                    Some(c) => c.clone(),
                    None if sig.requires_constraint => {
                        self.error(
                            "E0322",
                            format!("`{full}` must say what it is limited to"),
                            grant.span,
                            "add the path this grant covers",
                        );
                        self.note(format!(
                            "for example: allow {{ {full} \"/usr/bin/true\"; }}"
                        ));
                        continue;
                    }
                    None => String::new(),
                };
                let id = CapId(self.caps.len() as u32);
                self.cap_ids.insert(full.clone(), id);
                self.caps.push(CapEntry {
                    name: full,
                    kind: sig.kind,
                    constraint,
                    params: sig.params.to_vec(),
                    ret: sig.ret,
                });
            }
        }
    }

    // ---- pass 1: signatures ----

    fn collect_signatures(&mut self, module: &Module) {
        for (item_index, item) in module.items.iter().enumerate() {
            let Item::Fn(f) = item else {
                continue;
            };
            if let Some(prev) = self.fn_ids.get(&f.name.name) {
                let prev_span = self.fns[prev.index()].span;
                self.error(
                    "E0304",
                    format!("function `{}` is defined more than once", f.name.name),
                    f.name.span,
                    "redefined here",
                );
                self.diags
                    .last_mut()
                    .unwrap()
                    .secondary
                    .push(Label::new(prev_span, "first defined here"));
                let _ = prev_span;
                continue;
            }

            let params: Vec<TypeId> = f.params.iter().map(|p| self.resolve_type(&p.ty)).collect();
            let ret = f.ret.as_ref().map(|t| self.resolve_type(t));
            let id = FuncId(self.fns.len() as u32);
            self.fn_ids.insert(f.name.name.clone(), id);
            self.fns.push(FnState {
                name: f.name.name.clone(),
                local_types: params.clone(),
                params,
                ret,
                item_index,
                span: f.span,
            });
        }
    }

    fn resolve_type(&mut self, t: &TypeExpr) -> TypeId {
        if !t.args.is_empty() {
            self.error(
                "E0310",
                format!("`{}` takes no type arguments in v0.1", t.name.name),
                t.span,
                "type arguments are not supported yet",
            );
            return Types::ERROR;
        }
        match self.types.by_name(&t.name.name) {
            Some(id) => id,
            None => {
                self.error(
                    "E0310",
                    format!("unknown type `{}`", t.name.name),
                    t.span,
                    "not a known type",
                );
                self.note("v0.1 has Unit, Bool, Int, Float and String");
                Types::ERROR
            }
        }
    }

    // ---- pass 2: bodies ----

    fn check_bodies(&mut self, module: &Module) {
        for idx in 0..self.fns.len() {
            let item_index = self.fns[idx].item_index;
            let Item::Fn(decl) = &module.items[item_index] else {
                unreachable!("a function's item index must name a function");
            };
            self.check_fn(FuncId(idx as u32), decl);
        }
    }

    fn check_fn(&mut self, id: FuncId, decl: &FnDecl) {
        let state = &self.fns[id.index()];
        self.locals = state.params.clone();
        self.ret_ty = state.ret;
        self.ret_annotated = state.ret.is_some();

        self.scopes = vec![Vec::new()];
        for (i, p) in decl.params.iter().enumerate() {
            let slot = LocalId(i as u32);
            self.declare(&p.name.name, slot);
            self.res.insert(p.id, Res::Local(slot));
        }

        self.check_block(&decl.body);

        // An unannotated function with no `return` returns Unit.
        let ret = self.ret_ty.unwrap_or(Types::UNIT);
        if ret != Types::UNIT && !self.types.is_error(ret) && !always_returns(&decl.body) {
            let name = self.types.name(ret);
            self.error(
                "E0307",
                format!("`{}` must return {name} on every path", decl.name.name),
                decl.body.span,
                "control can reach the end of the body without returning",
            );
        }

        let locals = std::mem::take(&mut self.locals);
        let state = &mut self.fns[id.index()];
        state.ret = Some(ret);
        state.local_types = locals;
        self.scopes.clear();
    }

    fn declare(&mut self, name: &str, slot: LocalId) {
        self.scopes
            .last_mut()
            .expect("a scope must be open")
            .push((name.to_string(), slot));
    }

    /// Looks up a name in the innermost scope first, so an inner binding
    /// shadows an outer one.
    fn lookup(&self, name: &str) -> Option<LocalId> {
        for scope in self.scopes.iter().rev() {
            for (n, slot) in scope.iter().rev() {
                if n == name {
                    return Some(*slot);
                }
            }
        }
        None
    }

    fn new_local(&mut self, ty: TypeId) -> LocalId {
        let id = LocalId(self.locals.len() as u32);
        self.locals.push(ty);
        id
    }

    fn check_block(&mut self, block: &Block) {
        self.scopes.push(Vec::new());
        for stmt in &block.stmts {
            self.check_stmt(stmt);
        }
        self.scopes.pop();
    }

    fn check_stmt(&mut self, stmt: &Stmt) {
        match stmt {
            Stmt::Let {
                id,
                name,
                ty,
                init,
                span,
            } => {
                let init_ty = self.check_expr(init);
                let slot_ty = match ty {
                    Some(annotation) => {
                        let want = self.resolve_type(annotation);
                        self.expect_type(want, init_ty, init.span, "this initializer");
                        want
                    }
                    None => init_ty,
                };
                if slot_ty == Types::UNIT {
                    self.error(
                        "E0311",
                        format!("`{}` would have type Unit", name.name),
                        *span,
                        "a binding must hold a value",
                    );
                }
                let slot = self.new_local(slot_ty);
                self.declare(&name.name, slot);
                self.res.insert(*id, Res::Local(slot));
            }
            Stmt::Return { value, span, .. } => {
                let found = match value {
                    Some(e) => self.check_expr(e),
                    None => Types::UNIT,
                };
                let span = value.as_ref().map(|e| e.span).unwrap_or(*span);
                match self.ret_ty {
                    Some(want) => self.expect_type(want, found, span, "this value"),
                    None => {
                        // First `return` of an unannotated function fixes its type.
                        self.ret_ty = Some(found);
                    }
                }
            }
            Stmt::If(if_stmt) => self.check_if(if_stmt),
            Stmt::Expr { expr, .. } => {
                self.check_expr(expr);
            }
        }
    }

    fn check_if(&mut self, if_stmt: &IfStmt) {
        let cond = self.check_expr(&if_stmt.cond);
        self.expect_type(Types::BOOL, cond, if_stmt.cond.span, "this condition");
        self.check_block(&if_stmt.then_block);
        match if_stmt.else_branch.as_deref() {
            Some(ElseBranch::Block(b)) => self.check_block(b),
            Some(ElseBranch::If(inner)) => self.check_if(inner),
            None => {}
        }
    }

    /// Reports a mismatch unless either side is already an error.
    fn expect_type(&mut self, want: TypeId, found: TypeId, span: Span, what: &str) {
        if want == found || self.types.is_error(want) || self.types.is_error(found) {
            return;
        }
        let (w, f) = (self.types.name(want), self.types.name(found));
        self.error(
            "E0301",
            format!("expected {w}, found {f}"),
            span,
            format!("{what} has type {f}"),
        );
    }

    fn check_expr(&mut self, e: &Expr) -> TypeId {
        let ty = match &e.kind {
            ExprKind::Int(_) => Types::INT,
            ExprKind::Float(_) => Types::FLOAT,
            ExprKind::Bool(_) => Types::BOOL,
            ExprKind::Str(_) => Types::STR,
            ExprKind::Null => {
                self.error(
                    "E0312",
                    "`null` has no type in v0.1",
                    e.span,
                    "there is no optional type yet",
                );
                Types::ERROR
            }
            ExprKind::Path(name) => self.check_path(e.id, name),
            ExprKind::Unary { op, operand } => self.check_unary(*op, operand, e.span),
            ExprKind::Binary { op, lhs, rhs } => self.check_binary(*op, lhs, rhs, e.span),
            ExprKind::Call { callee, args } => self.check_call(callee, args, e.span),
            ExprKind::Field { base, name } => {
                if let Some(full) = self.capability_name(base, name) {
                    self.error(
                        "E0325",
                        format!("`{full}` is a capability and must be called"),
                        e.span,
                        "a capability is not a value",
                    );
                } else {
                    self.check_expr(base);
                    self.error(
                        "E0308",
                        "field access is not supported in v0.1",
                        e.span,
                        "there are no object types yet",
                    );
                }
                Types::ERROR
            }
            ExprKind::Error => Types::ERROR,
        };
        self.node_types.insert(e.id, ty);
        ty
    }

    fn check_path(&mut self, node: NodeId, name: &Ident) -> TypeId {
        if let Some(slot) = self.lookup(&name.name) {
            self.res.insert(node, Res::Local(slot));
            return self.locals[slot.index()];
        }
        if let Some(id) = self.fn_ids.get(&name.name).copied() {
            self.res.insert(node, Res::Fn(id));
            let sig = crate::ty::FnSig {
                params: self.fns[id.index()].params.clone(),
                ret: self.fns[id.index()].ret.unwrap_or(Types::ERROR),
            };
            let sig_id = self.types.add_sig(sig);
            return self.types.intern(Type::Fn(sig_id));
        }
        self.error(
            "E0300",
            format!("cannot find `{}`", name.name),
            name.span,
            "not defined in this scope",
        );
        Types::ERROR
    }

    fn check_unary(&mut self, op: UnOp, operand: &Expr, span: Span) -> TypeId {
        let ty = self.check_expr(operand);
        if self.types.is_error(ty) {
            return Types::ERROR;
        }
        let want = match op {
            UnOp::Neg => Types::INT,
            UnOp::Not => Types::BOOL,
        };
        if ty != want {
            let (t, w) = (self.types.name(ty), self.types.name(want));
            self.error(
                "E0303",
                format!("`{}` cannot be applied to {t}", op.text()),
                span,
                format!("expected {w}"),
            );
            if op == UnOp::Neg && ty == Types::FLOAT {
                self.note("v0.1 has integer arithmetic only");
            }
            return Types::ERROR;
        }
        want
    }

    fn check_binary(&mut self, op: BinOp, lhs: &Expr, rhs: &Expr, span: Span) -> TypeId {
        let l = self.check_expr(lhs);
        let r = self.check_expr(rhs);
        if self.types.is_error(l) || self.types.is_error(r) {
            return Types::ERROR;
        }
        if l != r {
            let (ln, rn) = (self.types.name(l), self.types.name(r));
            self.error(
                "E0303",
                format!("`{}` cannot be applied to {ln} and {rn}", op.text()),
                span,
                "both operands must have the same type",
            );
            self.note("there are no implicit conversions");
            return Types::ERROR;
        }

        // Both sides share the type `l` from here on.
        let ok = match op {
            BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem => l == Types::INT,
            BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => l == Types::INT,
            BinOp::Eq | BinOp::Ne => l == Types::INT || l == Types::BOOL,
            BinOp::And | BinOp::Or => l == Types::BOOL,
        };
        if !ok {
            let name = self.types.name(l);
            self.error(
                "E0303",
                format!("`{}` cannot be applied to {name}", op.text()),
                span,
                format!("no such operator for {name}"),
            );
            if l == Types::FLOAT || l == Types::STR {
                self.note("v0.1 supports arithmetic and comparison on Int only");
            }
            return Types::ERROR;
        }

        match op {
            BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem => l,
            _ => Types::BOOL,
        }
    }

    fn check_call(&mut self, callee: &Expr, args: &[Expr], span: Span) -> TypeId {
        // `fs.read(..)` parses as a call whose callee is a field access. That
        // shape is a capability call and nothing else, because there are no
        // object types to take a method from.
        if let ExprKind::Field { base, name } = &callee.kind {
            return self.check_cap_call(callee, base, name, args, span);
        }
        let ExprKind::Path(name) = &callee.kind else {
            for a in args {
                self.check_expr(a);
            }
            self.error(
                "E0305",
                "only a named function can be called in v0.1",
                callee.span,
                "not a function name",
            );
            return Types::ERROR;
        };

        // A local binding shadows a function, and no local can hold a function
        // in v0.1, so this is a call of something that is not callable.
        if self.lookup(&name.name).is_some() {
            for a in args {
                self.check_expr(a);
            }
            self.error(
                "E0305",
                format!("`{}` is a variable, not a function", name.name),
                callee.span,
                "cannot be called",
            );
            return Types::ERROR;
        }

        let Some(id) = self.fn_ids.get(&name.name).copied() else {
            for a in args {
                self.check_expr(a);
            }
            self.error(
                "E0300",
                format!("cannot find function `{}`", name.name),
                name.span,
                "not defined in this module",
            );
            return Types::ERROR;
        };
        self.res.insert(callee.id, Res::Fn(id));

        let params = self.fns[id.index()].params.clone();
        let ret = self.fns[id.index()].ret;

        if args.len() != params.len() {
            for a in args {
                self.check_expr(a);
            }
            self.error(
                "E0302",
                format!(
                    "`{}` takes {} argument(s) but {} were given",
                    name.name,
                    params.len(),
                    args.len()
                ),
                span,
                "wrong number of arguments",
            );
            return ret.unwrap_or(Types::ERROR);
        }

        for (arg, want) in args.iter().zip(params) {
            let found = self.check_expr(arg);
            self.expect_type(want, found, arg.span, "this argument");
        }

        match ret {
            Some(ty) => ty,
            None => {
                // Its return type is still being inferred, which means the call
                // is a forward or recursive reference.
                self.error(
                    "E0306",
                    format!("`{}` needs a return type annotation", name.name),
                    name.span,
                    "called before its type is known",
                );
                self.note("annotate it with `-> Type`, or define it before this call");
                Types::ERROR
            }
        }
    }

    /// The capability a `base.name` expression could name, if `base` is a plain
    /// identifier that no local binding shadows.
    fn capability_name(&self, base: &Expr, name: &Ident) -> Option<String> {
        let ExprKind::Path(ns) = &base.kind else {
            return None;
        };
        if self.lookup(&ns.name).is_some() {
            return None;
        }
        let full = format!("{}.{}", ns.name, name.name);
        cap::builtin(&full).map(|_| full)
    }

    fn check_cap_call(
        &mut self,
        callee: &Expr,
        base: &Expr,
        name: &Ident,
        args: &[Expr],
        span: Span,
    ) -> TypeId {
        let Some(full) = self.capability_name(base, name) else {
            for a in args {
                self.check_expr(a);
            }
            // Either the namespace is a local binding, or there is no such
            // capability; both mean this cannot be a capability call.
            if let ExprKind::Path(ns) = &base.kind {
                let attempted = format!("{}.{}", ns.name, name.name);
                if self.lookup(&ns.name).is_none() {
                    self.error(
                        "E0324",
                        format!("unknown capability `{attempted}`"),
                        span,
                        "no such capability",
                    );
                    self.note(format!("v0.1 has {}", cap::all_names().join(", ")));
                    return Types::ERROR;
                }
            }
            self.check_expr(base);
            self.error(
                "E0308",
                "field access is not supported in v0.1",
                callee.span,
                "there are no object types yet",
            );
            return Types::ERROR;
        };

        let Some(id) = self.cap_ids.get(&full).copied() else {
            for a in args {
                self.check_expr(a);
            }
            // The capability exists but the module never granted it. Declaring
            // it is the fix, which is why this is an error at compile time
            // rather than a refusal at run time.
            self.error(
                "E0320",
                format!("`{full}` is not allowed by this module"),
                span,
                "no grant covers this call",
            );
            self.note(format!("declare it: allow {{ {full} \"...\"; }}"));
            return Types::ERROR;
        };
        self.res.insert(callee.id, Res::Cap(id));

        let entry = &self.caps[id.index()];
        let (params, ret) = (entry.params.clone(), entry.ret);
        if args.len() != params.len() {
            for a in args {
                self.check_expr(a);
            }
            self.error(
                "E0302",
                format!(
                    "`{full}` takes {} argument(s) but {} were given",
                    params.len(),
                    args.len()
                ),
                span,
                "wrong number of arguments",
            );
            return ret;
        }
        for (arg, want) in args.iter().zip(params) {
            let found = self.check_expr(arg);
            self.expect_type(want, found, arg.span, "this argument");
        }
        ret
    }

    fn finish(self) -> (Typed, Vec<Diagnostic>) {
        let entry = self.fn_ids.get("main").copied();
        let fns = self
            .fns
            .into_iter()
            .map(|f| FnInfo {
                name: f.name,
                params: f.params,
                ret: f.ret.unwrap_or(Types::UNIT),
                local_types: f.local_types,
                item_index: f.item_index,
                span: f.span,
            })
            .collect();
        (
            Typed {
                types: self.types,
                fns,
                node_types: self.node_types,
                res: self.res,
                caps: self.caps,
                entry,
            },
            self.diags,
        )
    }
}

/// Whether every path through a block ends in `return`.
///
/// Statements after a `return` are unreachable rather than wrong, so this only
/// asks whether a `return` is guaranteed, not where it sits.
fn always_returns(block: &Block) -> bool {
    block.stmts.iter().any(stmt_always_returns)
}

fn stmt_always_returns(stmt: &Stmt) -> bool {
    match stmt {
        Stmt::Return { .. } => true,
        Stmt::If(if_stmt) => {
            let then_returns = always_returns(&if_stmt.then_block);
            let else_returns = match if_stmt.else_branch.as_deref() {
                Some(ElseBranch::Block(b)) => always_returns(b),
                Some(ElseBranch::If(inner)) => stmt_always_returns(&Stmt::If(inner.clone())),
                None => false,
            };
            then_returns && else_returns
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests;
