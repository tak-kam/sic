//! Types and the type checker.
//!
//! The checker also resolves names, because splitting resolution into its own
//! pass would buy nothing at the size of v0.1. Its output is a side table keyed
//! by `NodeId`: the AST is never modified.

pub mod cap;
pub mod check;
pub mod ty;

pub use cap::{BUILTIN_CAPS, CapEntry, CapSig, builtin};
pub use check::{Builtin, FnInfo, Res, Typed, check};
pub use ty::{FnSig, FnSigId, ObjectDef, ObjectId, TrustKind, Type, Types};
