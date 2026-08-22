//! The AST.
//!
//! The AST carries no analysis results such as types. Later layers keep side
//! tables keyed by `NodeId` instead, which means type checking and IR lowering
//! can evolve without reshaping the AST.

use sic_core::{NodeId, Span};

#[derive(Debug, Clone)]
pub struct Module {
    pub items: Vec<Item>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum Item {
    Fn(FnDecl),
}

impl Item {
    pub fn span(&self) -> Span {
        match self {
            Item::Fn(f) => f.span,
        }
    }
}

#[derive(Debug, Clone)]
pub struct FnDecl {
    pub id: NodeId,
    pub name: Ident,
    pub params: Vec<Param>,
    /// The return type annotation. When absent, type checking derives it from
    /// the body.
    pub ret: Option<TypeExpr>,
    pub body: Block,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct Param {
    pub id: NodeId,
    pub name: Ident,
    pub ty: TypeExpr,
    pub span: Span,
}

/// A type as written in the source. Resolving it to a `Type` is the job of
/// `sic-types`.
#[derive(Debug, Clone)]
pub struct TypeExpr {
    pub id: NodeId,
    pub name: Ident,
    pub args: Vec<TypeExpr>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct Ident {
    pub name: String,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct Block {
    pub id: NodeId,
    pub stmts: Vec<Stmt>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum Stmt {
    Let {
        id: NodeId,
        name: Ident,
        ty: Option<TypeExpr>,
        init: Expr,
        span: Span,
    },
    Return {
        id: NodeId,
        value: Option<Expr>,
        span: Span,
    },
    If(IfStmt),
    Expr {
        id: NodeId,
        expr: Expr,
        span: Span,
    },
}

impl Stmt {
    pub fn span(&self) -> Span {
        match self {
            Stmt::Let { span, .. } | Stmt::Return { span, .. } | Stmt::Expr { span, .. } => *span,
            Stmt::If(s) => s.span,
        }
    }
}

#[derive(Debug, Clone)]
pub struct IfStmt {
    pub id: NodeId,
    pub cond: Expr,
    pub then_block: Block,
    pub else_branch: Option<Box<ElseBranch>>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum ElseBranch {
    Block(Block),
    If(IfStmt),
}

#[derive(Debug, Clone)]
pub struct Expr {
    pub id: NodeId,
    pub kind: ExprKind,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum ExprKind {
    Int(i64),
    Float(f64),
    Bool(bool),
    Str(String),
    Null,
    /// A reference to a variable or function. There are no module paths in v0.1.
    Path(Ident),
    Unary {
        op: UnOp,
        operand: Box<Expr>,
    },
    Binary {
        op: BinOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    Call {
        callee: Box<Expr>,
        args: Vec<Expr>,
    },
    Field {
        base: Box<Expr>,
        name: Ident,
    },
    /// A hole produced by error recovery. Later layers stop analyzing here.
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnOp {
    Neg,
    Not,
}

impl UnOp {
    pub const fn text(self) -> &'static str {
        match self {
            UnOp::Neg => "-",
            UnOp::Not => "!",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
}

impl BinOp {
    pub const fn text(self) -> &'static str {
        match self {
            BinOp::Add => "+",
            BinOp::Sub => "-",
            BinOp::Mul => "*",
            BinOp::Div => "/",
            BinOp::Rem => "%",
            BinOp::Eq => "==",
            BinOp::Ne => "!=",
            BinOp::Lt => "<",
            BinOp::Le => "<=",
            BinOp::Gt => ">",
            BinOp::Ge => ">=",
            BinOp::And => "&&",
            BinOp::Or => "||",
        }
    }
}
