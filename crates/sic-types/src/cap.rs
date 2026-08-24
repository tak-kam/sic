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
    /// Whether a grant may pin the file's digest.
    ///
    /// Only running a program takes one. Pinning what `fs.read` reads would
    /// have to say what the contents must be, which is not what a grant is for,
    /// and accepting the syntax while ignoring it would be worse than refusing
    /// it.
    pub accepts_pin: bool,
    /// Whether the last parameter may be left off at the call site.
    ///
    /// Only `process.exec` has one: a call that passes no argument vector
    /// means an empty one, so every program written before arguments existed
    /// keeps saying what it said.
    pub optional_tail: bool,
}

/// What v0.1 can do. Nothing here needs a credential or a socket.
pub const BUILTIN_CAPS: &[CapSig] = &[
    CapSig {
        name: "fs.read",
        kind: CapKind::Read,
        params: &[Types::STR],
        ret: Types::STR,
        requires_constraint: true,
        accepts_pin: false,
        optional_tail: false,
    },
    CapSig {
        name: "fs.write",
        kind: CapKind::Write,
        params: &[Types::STR, Types::STR],
        ret: Types::UNIT,
        requires_constraint: true,
        accepts_pin: false,
        optional_tail: false,
    },
    CapSig {
        name: "llm.invoke",
        kind: CapKind::Invoke,
        // The prompt in, the raw answer out. Turning that answer into a value
        // is `from_json`, which is what an agent declaration wires up - and the
        // second argument is the shape that validation will insist on, so that
        // whoever answers is told what it has to be. An `agent` fills it in;
        // a direct call may leave it off.
        params: &[Types::STR, Types::STR],
        ret: Types::STR,
        // The constraint names the model, so a manifest says which one a
        // module may talk to.
        requires_constraint: true,
        accepts_pin: false,
        optional_tail: true,
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
        accepts_pin: false,
        optional_tail: false,
    },
    CapSig {
        name: "human.choose",
        kind: CapKind::Invoke,
        // The question, then the alternatives. What comes back is which one,
        // never what: see `docs/design/decisions.md`.
        params: &[Types::STR, Types::LIST_STR],
        ret: Types::INT,
        // An unconstrained decision would be a decision about anything, so a
        // grant has to say what it covers.
        requires_constraint: true,
        accepts_pin: false,
        optional_tail: false,
    },
    CapSig {
        name: "process.capture",
        // Running it is what it does; reading the answer is why. The kind is
        // what the trust rule looks at, and this one runs a program.
        kind: CapKind::Exec,
        params: &[Types::STR, Types::LIST_STR],
        // Only on a zero exit code: a program that failed did not produce an
        // answer worth reading. See `docs/design/output.md`.
        ret: Types::OBSERVED_STR,
        requires_constraint: true,
        accepts_pin: true,
        optional_tail: true,
    },
    CapSig {
        name: "process.exec",
        kind: CapKind::Exec,
        // The path, then what to pass it. The vector may be left off, and
        // leaving it off means passing nothing.
        params: &[Types::STR, Types::LIST_STR],
        ret: Types::INT,
        requires_constraint: true,
        // An absolute path says where to look, not what is there.
        accepts_pin: true,
        optional_tail: true,
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
    /// The digest the file has to have, or empty for a grant that does not pin.
    pub pin: String,
    /// What the argument vector has to start with, from `args [...]` on the
    /// grant. Empty means the call passes no arguments.
    pub args: Vec<String>,
    /// Whether the grant claims that performing this twice is the same as
    /// performing it once, from `repeatable`. Without it, `retry` on a call to
    /// this capability does not compile.
    pub repeatable: bool,
    pub params: Vec<TypeId>,
    /// From the signature: whether the last parameter may be left off.
    pub optional_tail: bool,
    pub ret: TypeId,
}
