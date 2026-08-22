//! HIR data types.

use sic_core::{BlockId, CapId, ConstIdx, FuncId, LocalId, Span, TypeId};

// The operator enums are shared with the AST rather than duplicated. Re-exported
// here so that later phases depend on the IR, not on the syntax layer.
pub use sic_syntax::ast::{BinOp, UnOp};

#[derive(Debug, Clone)]
pub struct Hir {
    pub funcs: Vec<HirFunc>,
    pub consts: Vec<Const>,
}

#[derive(Debug, Clone)]
pub struct HirFunc {
    pub name: String,
    /// Parameters occupy locals 0..params.len().
    pub params: Vec<LocalId>,
    pub ret: TypeId,
    /// The type of every local, indexed by `LocalId`.
    pub locals: Vec<TypeId>,
    pub blocks: Vec<HirBlock>,
    pub entry: BlockId,
}

impl HirFunc {
    pub fn block(&self, id: BlockId) -> &HirBlock {
        &self.blocks[id.index()]
    }
}

#[derive(Debug, Clone)]
pub struct HirBlock {
    pub id: BlockId,
    pub insts: Vec<Inst>,
    pub term: Terminator,
}

/// A constant in the pool shared by the whole module.
#[derive(Debug, Clone, PartialEq)]
pub enum Const {
    Unit,
    Bool(bool),
    I64(i64),
    F64(f64),
    Str(String),
}

/// An instruction and the source it came from. The span is what later fills the
/// debug section of the bytecode file.
#[derive(Debug, Clone)]
pub struct Inst {
    pub kind: InstKind,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum InstKind {
    Const {
        dst: LocalId,
        k: ConstIdx,
    },
    Move {
        dst: LocalId,
        src: LocalId,
    },
    Un {
        dst: LocalId,
        op: UnOp,
        x: LocalId,
    },
    Bin {
        dst: LocalId,
        op: BinOp,
        l: LocalId,
        r: LocalId,
    },
    Call {
        dst: LocalId,
        func: FuncId,
        args: Vec<LocalId>,
    },

    // ---- phase 3 and later ----
    // Defined here so the shape of the IR is settled, never generated in v0.1.
    /// A capability call: the only way to reach the outside world.
    CallCap {
        dst: LocalId,
        cap: CapId,
        args: Vec<LocalId>,
        policy: CallPolicy,
    },
    Spawn {
        dst: LocalId,
        func: FuncId,
        args: Vec<LocalId>,
    },
    Await {
        dst: LocalId,
        task: LocalId,
    },
    Log {
        level: LogLevel,
        msg: LocalId,
        fields: Vec<(String, LocalId)>,
    },
}

#[derive(Debug, Clone)]
pub struct Terminator {
    pub kind: Term,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum Term {
    Jump(BlockId),
    Branch {
        cond: LocalId,
        then_bb: BlockId,
        else_bb: BlockId,
    },
    Return(Option<LocalId>),
    Fail(LocalId),
}

/// Workflow semantics attached to a capability call. Never lowered into VM
/// instructions; the runtime reads it for scheduling, and `sic plan` reads it to
/// describe what a program would do.
#[derive(Debug, Clone, Default)]
pub struct CallPolicy {
    pub retry: Option<RetrySpec>,
    pub timeout: Option<std::time::Duration>,
    pub budget: Option<BudgetSpec>,
    pub idempotency_key: Option<LocalId>,
}

#[derive(Debug, Clone)]
pub struct RetrySpec {
    pub attempts: u32,
    pub backoff: Backoff,
}

#[derive(Debug, Clone)]
pub enum Backoff {
    Fixed(std::time::Duration),
    Exponential {
        base: std::time::Duration,
        factor: u32,
    },
}

#[derive(Debug, Clone)]
pub struct BudgetSpec {
    pub kind: BudgetKind,
    pub limit: u64,
}

#[derive(Debug, Clone, Copy)]
pub enum BudgetKind {
    Tokens,
    Cost,
    Calls,
    Duration,
}

#[derive(Debug, Clone, Copy)]
pub enum LogLevel {
    Debug,
    Info,
    Warn,
    Error,
}
