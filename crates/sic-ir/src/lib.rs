//! The high-level IR.
//!
//! This is the layer that still knows about workflow semantics. Control flow is
//! already basic blocks and terminators, but retry, timeout, parallelism,
//! agents, approvals and budgets survive as instructions, because `sic plan`
//! has to be able to reconstruct a plan from what is compiled.

pub mod hir;
pub mod lower;
pub mod print;

pub use hir::{BinOp, Const, Hir, HirBlock, HirFunc, Inst, InstKind, Term, Terminator, UnOp};
pub use lower::lower;
pub use print::dump;
