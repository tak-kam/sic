//! An S-expression dump of the AST.
//!
//! Used by `sic parse` and by the parser tests. It only has to be readable and
//! deterministic down to the whitespace.

use crate::ast::*;

pub fn dump(m: &Module) -> String {
    let mut p = Printer {
        out: String::new(),
        depth: 0,
    };
    p.line("(module");
    p.depth += 1;
    for item in &m.items {
        match item {
            Item::Fn(f) => p.fn_decl(f),
        }
    }
    p.depth -= 1;
    p.push_close();
    p.out
}

struct Printer {
    out: String,
    depth: usize,
}

impl Printer {
    fn line(&mut self, s: &str) {
        for _ in 0..self.depth {
            self.out.push_str("  ");
        }
        self.out.push_str(s);
        self.out.push('\n');
    }

    /// Appends `)` to the previous line rather than emitting a line of its own.
    fn push_close(&mut self) {
        while self.out.ends_with('\n') {
            self.out.pop();
        }
        self.out.push_str(")\n");
    }

    fn fn_decl(&mut self, f: &FnDecl) {
        let ret = f.ty_str();
        self.line(&format!("(fn {}{ret}", f.name.name));
        self.depth += 1;
        if !f.params.is_empty() {
            let ps: Vec<String> = f
                .params
                .iter()
                .map(|p| format!("({} {})", p.name.name, type_str(&p.ty)))
                .collect();
            self.line(&format!("(params {})", ps.join(" ")));
        }
        self.block(&f.body);
        self.depth -= 1;
        self.push_close();
    }

    fn block(&mut self, b: &Block) {
        if b.stmts.is_empty() {
            self.line("(block)");
            return;
        }
        self.line("(block");
        self.depth += 1;
        for s in &b.stmts {
            self.stmt(s);
        }
        self.depth -= 1;
        self.push_close();
    }

    fn stmt(&mut self, s: &Stmt) {
        match s {
            Stmt::Let { name, ty, init, .. } => {
                let t = ty
                    .as_ref()
                    .map(|t| format!(": {}", type_str(t)))
                    .unwrap_or_default();
                self.line(&format!("(let {}{t} {})", name.name, expr_str(init)));
            }
            Stmt::Return { value, .. } => match value {
                Some(e) => self.line(&format!("(return {})", expr_str(e))),
                None => self.line("(return)"),
            },
            Stmt::Expr { expr, .. } => self.line(&format!("(expr {})", expr_str(expr))),
            Stmt::If(i) => self.if_stmt(i),
        }
    }

    fn if_stmt(&mut self, i: &IfStmt) {
        self.line(&format!("(if {}", expr_str(&i.cond)));
        self.depth += 1;
        self.block(&i.then_block);
        match i.else_branch.as_deref() {
            Some(ElseBranch::Block(b)) => {
                self.line("(else");
                self.depth += 1;
                self.block(b);
                self.depth -= 1;
                self.push_close();
            }
            Some(ElseBranch::If(inner)) => {
                self.line("(else");
                self.depth += 1;
                self.if_stmt(inner);
                self.depth -= 1;
                self.push_close();
            }
            None => {}
        }
        self.depth -= 1;
        self.push_close();
    }
}

impl FnDecl {
    fn ty_str(&self) -> String {
        match &self.ret {
            Some(t) => format!(" -> {}", type_str(t)),
            None => String::new(),
        }
    }
}

fn type_str(t: &TypeExpr) -> String {
    if t.args.is_empty() {
        t.name.name.clone()
    } else {
        let args: Vec<String> = t.args.iter().map(type_str).collect();
        format!("{}<{}>", t.name.name, args.join(", "))
    }
}

/// Expressions are printed on one line; parentheses show the tree shape.
pub fn expr_str(e: &Expr) -> String {
    match &e.kind {
        ExprKind::Int(v) => format!("{v}"),
        ExprKind::Float(v) => {
            // Print a trailing `.0` so a whole number still reads as a float.
            if v.fract() == 0.0 && v.is_finite() {
                format!("{v:.1}")
            } else {
                format!("{v}")
            }
        }
        ExprKind::Bool(v) => format!("{v}"),
        ExprKind::Str(s) => format!("{s:?}"),
        ExprKind::Null => "null".into(),
        ExprKind::Path(i) => i.name.clone(),
        ExprKind::Unary { op, operand } => format!("({} {})", op.text(), expr_str(operand)),
        ExprKind::Binary { op, lhs, rhs } => {
            format!("({} {} {})", op.text(), expr_str(lhs), expr_str(rhs))
        }
        ExprKind::Call { callee, args } => {
            let a: Vec<String> = args.iter().map(expr_str).collect();
            format!("(call {} {})", expr_str(callee), a.join(" ")).replace(" )", ")")
        }
        ExprKind::Field { base, name } => format!("(. {} {})", expr_str(base), name.name),
        ExprKind::Error => "<error>".into(),
    }
}
