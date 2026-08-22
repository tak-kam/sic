//! The minimal foundation shared by every crate in the sic implementation.
//!
//! Only things whose meaning is the same at every layer belong here. Knowledge of
//! syntax, types, IR, or the VM must not leak into this crate.

pub mod diag;
pub mod id;
pub mod span;

pub use diag::{Diagnostic, Label, Severity};
pub use id::{BlockId, CapId, ConstIdx, FuncId, LocalId, NodeId, TypeId};
pub use span::{LineCol, SourceFile, Span};
