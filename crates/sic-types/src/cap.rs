//! The capabilities the language knows about, and the table a module builds
//! from its `allow` block.
//!
//! Capabilities are built in rather than user-defined. Letting a program name
//! an effect the runtime has never heard of would make the manifest a
//! suggestion instead of a contract.

use sic_core::{CapKind, TypeId};

use crate::ty::Types;

/// The signature of a built-in capability.
#[derive(Debug, Clone, Copy)]
pub struct CapSig {
    pub name: &'static str,
    pub kind: CapKind,
    pub params: &'static [TypeId],
    pub ret: TypeId,
    /// Whether a grant must name what it is limited to. Every capability in
    /// v0.1 does: an unconstrained grant of `process.exec` is a shell.
    pub requires_constraint: bool,
}

/// What v0.1 can do. Nothing here needs a credential or a socket.
pub const BUILTIN_CAPS: &[CapSig] = &[
    CapSig {
        name: "fs.read",
        kind: CapKind::Read,
        params: &[Types::STR],
        ret: Types::STR,
        requires_constraint: true,
    },
    CapSig {
        name: "fs.write",
        kind: CapKind::Write,
        params: &[Types::STR, Types::STR],
        ret: Types::UNIT,
        requires_constraint: true,
    },
    CapSig {
        name: "human.approve",
        kind: CapKind::Invoke,
        // The question is the argument; the answer is whether it was approved.
        params: &[Types::STR],
        ret: Types::BOOL,
        // An unconstrained approval would be an approval of anything, so a
        // grant has to say what it covers.
        requires_constraint: true,
    },
    CapSig {
        name: "process.exec",
        kind: CapKind::Exec,
        // No argument vector until there is a list type; the result is the
        // exit code.
        params: &[Types::STR],
        ret: Types::INT,
        requires_constraint: true,
    },
];

pub fn builtin(full_name: &str) -> Option<&'static CapSig> {
    BUILTIN_CAPS.iter().find(|c| c.name == full_name)
}

/// Every capability name, for a diagnostic that lists the alternatives.
pub fn all_names() -> Vec<&'static str> {
    BUILTIN_CAPS.iter().map(|c| c.name).collect()
}

/// One entry of a module's manifest: a capability it granted itself.
#[derive(Debug, Clone)]
pub struct CapEntry {
    pub name: String,
    pub kind: CapKind,
    pub constraint: String,
    pub params: Vec<TypeId>,
    pub ret: TypeId,
}
