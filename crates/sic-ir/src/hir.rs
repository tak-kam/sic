//! HIR data types.

use sic_core::{BlockId, CapId, ConstIdx, FuncId, LocalId, Span, TypeId};

// The operator enums are shared with the AST rather than duplicated. Re-exported
// here so that later phases depend on the IR, not on the syntax layer.
pub use sic_syntax::ast::{BinOp, UnOp};

#[derive(Debug, Clone)]
pub struct Hir {
    pub funcs: Vec<HirFunc>,
    pub consts: Vec<Const>,
    /// The capabilities the module granted itself, in manifest order. A
    /// `CallCap` indexes into this.
    pub caps: Vec<sic_types::CapEntry>,
    /// The type table the ids in this module refer to.
    pub types: sic_types::Types,
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
    /// Builds a record of type `ty` from its fields, in declaration order.
    MakeObject {
        dst: LocalId,
        ty: TypeId,
        fields: Vec<LocalId>,
    },
    /// Reads the field at `index`, which the type checker resolved from a name.
    GetField {
        dst: LocalId,
        base: LocalId,
        index: u32,
    },
    MakeList {
        dst: LocalId,
        ty: TypeId,
        elements: Vec<LocalId>,
    },
    GetIndex {
        dst: LocalId,
        base: LocalId,
        index: LocalId,
    },
    /// The length of a list or a string.
    Len {
        dst: LocalId,
        src: LocalId,
    },
    /// Parses a document and checks it against a type.
    ///
    /// This is where a model's answer stops being text. A run fails here rather
    /// than wherever the malformed value would first have been used.
    FromJson {
        dst: LocalId,
        ty: TypeId,
        src: LocalId,
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

/// Workflow semantics attached to a capability call.
///
/// This never becomes a VM instruction. Retry is carried out by the VM, which
/// re-issues the request and records every attempt; the timeout travels to the
/// broker, which is the only side with a clock. `sic plan` reads the same
/// information without executing anything.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CallPolicy {
    /// Total attempts, not extra ones.
    pub attempts: Option<u32>,
    pub timeout_ms: Option<u32>,
    /// Phase 7.
    pub budget: Option<BudgetSpec>,
    /// Phase 7: what makes a retry safe to repeat.
    pub idempotency_key: Option<LocalId>,
}

impl CallPolicy {
    pub fn is_empty(&self) -> bool {
        self.attempts.is_none()
            && self.timeout_ms.is_none()
            && self.budget.is_none()
            && self.idempotency_key.is_none()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BudgetSpec {
    pub kind: BudgetKind,
    pub limit: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
