//! The AST.
//!
//! The AST carries no analysis results such as types. Later layers keep side
//! tables keyed by `NodeId` instead, which means type checking and IR lowering
//! can evolve without reshaping the AST.

use sic_core::{Answers, NodeId, Span};

#[derive(Debug, Clone)]
pub struct Module {
    pub items: Vec<Item>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum Item {
    Fn(FnDecl),
    /// A block of capability grants. Declaring a capability is what makes
    /// calling it legal, so this is part of the program, not configuration.
    Allow(AllowDecl),
    /// A user-defined record type.
    Type(TypeDecl),
    /// An agent: a model call and the shape its answer has to fit.
    Agent(AgentDecl),
    /// Another file, brought in whole.
    Import(ImportDecl),
    /// What an imported file needs the program to allow.
    Requires(RequiresDecl),
}

impl Item {
    pub fn span(&self) -> Span {
        match self {
            Item::Fn(f) => f.span,
            Item::Allow(a) => a.span,
            Item::Type(t) => t.span,
            Item::Agent(a) => a.span,
            Item::Import(i) => i.span,
            Item::Requires(r) => r.span,
        }
    }
}

/// `import "./lib/deploy.sic";`
#[derive(Debug, Clone)]
pub struct ImportDecl {
    pub id: NodeId,
    /// The path as written, relative to the importing file.
    pub path: String,
    pub span: Span,
}

/// ```text
/// requires {
///     process.exec;
/// }
/// ```
///
/// A capability, never a constraint: what a library does is its own business,
/// which file or binary it is pointed at is the program's.
#[derive(Debug, Clone)]
pub struct RequiresDecl {
    pub id: NodeId,
    pub caps: Vec<CapPath>,
    pub span: Span,
}

/// ```text
/// agent diagnose {
///     input: String,
///     output: Diagnosis,
///     budget: 8,
/// }
/// ```
#[derive(Debug, Clone)]
pub struct AgentDecl {
    pub id: NodeId,
    pub name: Ident,
    pub input: Option<TypeExpr>,
    pub output: Option<TypeExpr>,
    /// How many times the agent may call its model in a whole run.
    pub budget: Option<u32>,
    /// Whether the agent keeps one conversation for as long as a task, instead
    /// of starting a fresh one every call. Written `memory: task`.
    ///
    /// There is deliberately no value meaning the default: a word whose only
    /// use is to say "the usual" is vocabulary that earns nothing, and the
    /// absence of the field already reads as what it means.
    pub memory: bool,
    /// How many tools the agent may use, at this call site, in a whole run.
    pub tools: Option<u32>,
    /// How long it has to produce one answer, in milliseconds.
    ///
    /// The same unit as `timeout`, which is the only unit any duration in this
    /// language has. It reads badly at this magnitude and that is the lesser
    /// evil: two units in one file is a bug nobody sees.
    pub deadline_ms: Option<u32>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct TypeDecl {
    pub id: NodeId,
    pub name: Ident,
    /// Fields are ordered. The order is what the bytecode uses; the source uses
    /// names.
    pub fields: Vec<FieldDecl>,
    /// `..` at the end of the body: the type describes part of a document
    /// rather than all of it, so `from_json` ignores a field it does not
    /// declare. Without it a document with an extra field is refused, which is
    /// what a model's answer needs. See `docs/design/agents.md` §8.
    pub open: bool,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct FieldDecl {
    pub id: NodeId,
    pub name: Ident,
    pub ty: TypeExpr,
    pub span: Span,
}

/// One field of a struct literal.
#[derive(Debug, Clone)]
pub struct FieldInit {
    pub name: Ident,
    pub value: Expr,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct AllowDecl {
    pub id: NodeId,
    pub grants: Vec<CapGrant>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct CapGrant {
    pub id: NodeId,
    pub path: CapPath,
    /// What the grant is limited to: a file path, an executable path. Its
    /// meaning belongs to the capability, not to the grammar.
    pub constraint: Option<String>,
    /// The digest the file has to have, for a grant that pins what runs.
    pub sha256: Option<Ident2>,
    /// What the argument vector has to start with, from `args ["a", "b"]`.
    /// Absent and empty mean the same thing: the call passes no arguments.
    pub args: Vec<Ident2>,
    /// Whether the grant says performing this twice is the same as performing
    /// it once, from `repeatable`. Without it, `retry` on a call to this
    /// capability does not compile.
    pub repeatable: bool,
    /// Whether the grant says an agent answering this program's model calls
    /// may use it too, from `delegable`. Without it the capability is the
    /// program's and not the agent's. See `docs/design/authority.md`.
    pub delegable: bool,
    /// The directory the child runs in, from `in "/abs/path"`. Absent means
    /// the one `sic` itself was started in.
    pub dir: Option<Ident2>,
    /// The environment the child is given, from `env { NAME: "value" }`.
    /// Absent and empty mean the same thing: no environment at all.
    pub env: Vec<(Ident2, Ident2)>,
    /// What shape the grant says the program answers in, from `answers json`.
    /// Absent means the grant claims nothing about it, which is what every
    /// grant claimed before this existed.
    pub answers: Option<AnswersClause>,
    pub span: Span,
}

/// The format a grant named, and where it named it.
///
/// The span is carried because the word is checked twice: the parser refuses
/// one that is neither `json` nor `jsonl`, and the checker refuses either on a
/// capability with no output to shape.
#[derive(Debug, Clone)]
pub struct AnswersClause {
    pub shape: Answers,
    pub span: Span,
}

/// A string written in the source, with where it was written.
#[derive(Debug, Clone)]
pub struct Ident2 {
    pub text: String,
    pub span: Span,
}

/// A capability name, always `namespace.name`.
#[derive(Debug, Clone)]
pub struct CapPath {
    pub namespace: Ident,
    pub name: Ident,
    pub span: Span,
}

impl CapPath {
    pub fn full_name(&self) -> String {
        format!("{}.{}", self.namespace.name, self.name.name)
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
    /// `for x in xs { ... }` - the only loop in the language.
    ///
    /// Over a list and nothing else: the count is `len(xs)`, fixed when the
    /// loop starts, so there is no way to write one that does not end. See
    /// `docs/design/v0.1.md` §2.
    For(ForStmt),
    Expr {
        id: NodeId,
        expr: Expr,
        span: Span,
    },
    /// `log info "starting";` - what the program has to say about itself.
    ///
    /// A statement rather than a capability: it reaches nothing outside the
    /// run's own account of itself, so a grant would be a grant to do nothing.
    /// See `docs/design/logging.md`.
    Log {
        id: NodeId,
        level: LogLevel,
        message: Expr,
        span: Span,
    },
}

/// How much a line matters, as the program says it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    Debug,
    Info,
    Warn,
    Error,
}

impl LogLevel {
    /// The four, as they are written. Ordinary identifiers rather than
    /// keywords, like `args` and `repeatable`: nothing is reserved for them.
    pub fn from_name(name: &str) -> Option<LogLevel> {
        Some(match name {
            "debug" => LogLevel::Debug,
            "info" => LogLevel::Info,
            "warn" => LogLevel::Warn,
            "error" => LogLevel::Error,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            LogLevel::Debug => "debug",
            LogLevel::Info => "info",
            LogLevel::Warn => "warn",
            LogLevel::Error => "error",
        }
    }
}

impl Stmt {
    pub fn span(&self) -> Span {
        match self {
            Stmt::Let { span, .. }
            | Stmt::Return { span, .. }
            | Stmt::Expr { span, .. }
            | Stmt::Log { span, .. } => *span,
            Stmt::If(s) => s.span,
            Stmt::For(s) => s.span,
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

/// `for x in xs { ... }`.
///
/// The binding is immutable and scoped to the body, exactly like a `let`, and
/// the iterable is an expression rather than a range because a list is the only
/// thing there is to walk.
#[derive(Debug, Clone)]
pub struct ForStmt {
    pub id: NodeId,
    pub var: Ident,
    pub iter: Expr,
    pub body: Block,
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
        /// How the call is to be retried and how long it may take. Only a
        /// capability call may carry one; the checker enforces that.
        policy: CallPolicy,
    },
    /// `spawn f(args)`: starts a task and evaluates to `Task<R>`.
    Spawn {
        callee: Box<Expr>,
        args: Vec<Expr>,
    },
    /// `await t`: waits for a task and evaluates to its result.
    Await {
        task: Box<Expr>,
    },
    /// `Point { x: 1, y: 2 }`.
    Struct {
        name: Ident,
        fields: Vec<FieldInit>,
    },
    /// `[1, 2, 3]`.
    List {
        elements: Vec<Expr>,
    },
    /// `xs[i]`.
    Index {
        base: Box<Expr>,
        index: Box<Expr>,
    },
    Field {
        base: Box<Expr>,
        name: Ident,
    },
    /// A hole produced by error recovery. Later layers stop analyzing here.
    Error,
}

/// A retry and timeout policy written after a call.
///
/// Both are optional and either may be written first, so that reading a call
/// site never depends on remembering an order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CallPolicy {
    /// Total attempts, not extra ones. `retry 1` is the same as no retry.
    pub attempts: Option<u32>,
    pub timeout_ms: Option<u32>,
    /// The span of the policy itself, for diagnostics about where it may go.
    pub span: Option<Span>,
}

impl CallPolicy {
    pub fn is_empty(&self) -> bool {
        self.attempts.is_none() && self.timeout_ms.is_none()
    }
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
