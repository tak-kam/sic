//! The types that cross the boundary between the VM and the capability broker.
//!
//! They live in `sic-core` because both sides need them and neither may depend
//! on the other: the VM must not be able to reach an implementation of an
//! effect, and the broker must not be able to reach into the VM's state.
//!
//! This is the future IPC boundary, so nothing here refers to VM memory. A
//! `CapValue` owns its data and can be written out and read back.

/// What kind of effect a capability has. This is what a plan or an audit log
/// summarizes, so it is coarse on purpose.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum CapKind {
    Read = 0,
    Write = 1,
    Exec = 2,
    Invoke = 3,
}

impl CapKind {
    pub fn from_u8(v: u8) -> Option<CapKind> {
        Some(match v {
            0 => CapKind::Read,
            1 => CapKind::Write,
            2 => CapKind::Exec,
            3 => CapKind::Invoke,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            CapKind::Read => "read",
            CapKind::Write => "write",
            CapKind::Exec => "exec",
            CapKind::Invoke => "invoke",
        }
    }
}

/// A value passed to or returned from a capability.
#[derive(Debug, Clone, PartialEq)]
pub enum CapValue {
    Unit,
    Bool(bool),
    I64(i64),
    F64(f64),
    Str(String),
}

impl CapValue {
    pub fn type_name(&self) -> &'static str {
        match self {
            CapValue::Unit => "Unit",
            CapValue::Bool(_) => "Bool",
            CapValue::I64(_) => "Int",
            CapValue::F64(_) => "Float",
            CapValue::Str(_) => "String",
        }
    }

    /// The string behind a `Str`, for a broker that expects one.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            CapValue::Str(s) => Some(s),
            _ => None,
        }
    }
}

/// One entry of a module's manifest, as the broker sees it.
///
/// The broker is given the manifest, not the bytecode: it needs to know what
/// was granted, and nothing about how the module is compiled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapGrant {
    pub name: String,
    pub kind: CapKind,
    /// What the grant is limited to. Its meaning belongs to the capability.
    pub constraint: String,
}

/// What the VM asks the broker to do.
#[derive(Debug, Clone, PartialEq)]
pub struct CapRequest {
    /// Index into the module's capability manifest. The broker checks the
    /// request against that entry rather than trusting the name.
    pub index: u32,
    pub name: String,
    pub args: Vec<CapValue>,
}

/// Why a capability call did not happen, or did not succeed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapError {
    pub message: String,
}

impl CapError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for CapError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
