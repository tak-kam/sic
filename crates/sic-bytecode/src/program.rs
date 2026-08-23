//! The in-memory form of a bytecode module.

use crate::inst::Inst;

// The kind of an effect means the same thing to the compiler, the verifier, the
// VM and the broker, so it is defined once in sic-core.
pub use sic_core::CapKind;

/// A decoded module. Producing one says nothing about whether it is safe to
/// run; that is what `sic-verify` decides.
#[derive(Debug, Clone, Default)]
pub struct Program {
    pub consts: Vec<Const>,
    /// Type descriptors, referenced by index from the function and capability
    /// tables.
    pub types: Vec<TypeDesc>,
    pub funcs: Vec<FuncDef>,
    /// The capability manifest. Empty in v0.1; filled in phase 3.
    pub caps: Vec<CapDecl>,
    /// Every function's instructions, concatenated.
    pub code: Vec<Inst>,
    /// Retry and timeout, per capability call site. Keyed by pc rather than
    /// encoded in the instruction, which has no room for it, and readable
    /// without executing anything.
    pub policies: Vec<PolicyEntry>,
    pub debug: DebugInfo,
}

/// The policy attached to one `CALL_CAP` site.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PolicyEntry {
    /// The instruction this applies to.
    pub pc: u32,
    /// Total attempts, not extra ones. Always at least 1.
    pub attempts: u32,
    /// Milliseconds, or 0 for no deadline.
    pub timeout_ms: u32,
    /// How many times this call site may run in a whole run, or 0 for no
    /// limit. Counting calls is what can be enforced honestly today; tokens and
    /// cost need the broker to report them.
    pub budget: u32,
}

impl Program {
    /// How a type is spelled, following task types into the table.
    pub fn type_name(&self, index: u32) -> String {
        match self.types.get(index as usize) {
            Some(TypeDesc::Task(inner)) => format!("Task<{}>", self.type_name(*inner)),
            Some(TypeDesc::List(inner)) => format!("List<{}>", self.type_name(*inner)),
            Some(other) => other.short_name().to_string(),
            None => "?".to_string(),
        }
    }

    /// The policy for a capability call site, if it has one.
    pub fn policy_at(&self, pc: u32) -> Option<PolicyEntry> {
        self.policies.iter().find(|p| p.pc == pc).copied()
    }

    /// Looks up a function by name, which is how the CLI finds `main`.
    pub fn func_by_name(&self, name: &str) -> Option<u32> {
        self.funcs
            .iter()
            .position(|f| f.name == name)
            .map(|i| i as u32)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Const {
    Unit,
    Bool(bool),
    I64(i64),
    F64(f64),
    Str(String),
    /// An empty list of the type at this index.
    ///
    /// `MAKE_LIST` takes its element type from the elements, which an empty
    /// list does not have. It is a constant rather than an instruction because
    /// a list cannot be modified, so one empty list can be shared.
    EmptyList(u32),
}

impl Const {
    pub fn tag(&self) -> u8 {
        match self {
            Const::Unit => 0,
            Const::Bool(_) => 1,
            Const::I64(_) => 2,
            Const::F64(_) => 3,
            Const::Str(_) => 4,
            Const::EmptyList(_) => 5,
        }
    }

    /// The type a constant has once loaded, used by the verifier.
    pub fn type_desc(&self) -> TypeDesc {
        match self {
            Const::Unit => TypeDesc::Unit,
            Const::Bool(_) => TypeDesc::Bool,
            Const::I64(_) => TypeDesc::Int,
            Const::F64(_) => TypeDesc::Float,
            Const::Str(_) => TypeDesc::Str,
            // The list's own type index is what it carries; the caller uses
            // `Const::list_type` for it.
            Const::EmptyList(_) => TypeDesc::Unit,
        }
    }

    /// The type index of an empty list constant.
    pub fn list_type(&self) -> Option<u32> {
        match self {
            Const::EmptyList(index) => Some(*index),
            _ => None,
        }
    }
}

/// The types the bytecode level distinguishes.
///
/// Coarser than the source language: the verifier only has to keep the VM's
/// assumptions true, so it tracks representations rather than the full type
/// system. `Task` is the exception, because the verifier has to know what
/// `AWAIT` produces, and that is why the type section holds descriptors rather
/// than single bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeDesc {
    Unit,
    Bool,
    Int,
    Float,
    Str,
    /// A task, and the index of the type it produces.
    Task(u32),
    /// A list, and the index of the type it holds.
    List(u32),
    /// A record: its name and its fields in order.
    ///
    /// Instructions address a field by position - the compiler knows the
    /// layout, and a verifier comparing names would be doing the type checker's
    /// work again. The names are here because validating a JSON document needs
    /// them: a document addresses fields by name.
    Object {
        name: String,
        fields: Vec<(String, u32)>,
    },
}

impl TypeDesc {
    /// The first five entries of the type section are the primitives, in tag
    /// order, so a primitive is its own index.
    pub const PRIMITIVES: [TypeDesc; 5] = [
        TypeDesc::Unit,
        TypeDesc::Bool,
        TypeDesc::Int,
        TypeDesc::Float,
        TypeDesc::Str,
    ];

    pub fn primitives() -> Vec<TypeDesc> {
        Self::PRIMITIVES.to_vec()
    }

    pub fn tag(&self) -> u8 {
        match self {
            TypeDesc::Unit => 0,
            TypeDesc::Bool => 1,
            TypeDesc::Int => 2,
            TypeDesc::Float => 3,
            TypeDesc::Str => 4,
            TypeDesc::Task(_) => 5,
            TypeDesc::List(_) => 6,
            TypeDesc::Object { .. } => 7,
        }
    }

    /// The index a primitive occupies in the type section.
    pub fn primitive_index(&self) -> Option<u32> {
        match self {
            TypeDesc::Task(_) | TypeDesc::List(_) | TypeDesc::Object { .. } => None,
            other => Some(other.tag() as u32),
        }
    }

    pub fn short_name(&self) -> &str {
        match self {
            TypeDesc::Unit => "Unit",
            TypeDesc::Bool => "Bool",
            TypeDesc::Int => "Int",
            TypeDesc::Float => "Float",
            TypeDesc::Str => "String",
            TypeDesc::Task(_) => "Task",
            TypeDesc::List(_) => "List",
            TypeDesc::Object { name, .. } => name,
        }
    }

    /// The fields of a record, if this is one.
    pub fn fields(&self) -> Option<&[(String, u32)]> {
        match self {
            TypeDesc::Object { fields, .. } => Some(fields),
            _ => None,
        }
    }

    /// The type of each field, in order.
    pub fn field_types(&self) -> Option<Vec<u32>> {
        self.fields()
            .map(|fields| fields.iter().map(|(_, t)| *t).collect())
    }
}

#[derive(Debug, Clone)]
pub struct FuncDef {
    pub name: String,
    /// The type of each parameter, as an index into `Program::types`.
    ///
    /// The verifier needs these to know the state of the entry frame and to
    /// check the arguments at a call site, so they are part of the format
    /// rather than something a caller has to supply.
    pub params: Vec<u32>,
    /// How many registers the frame needs. The verifier checks that no
    /// instruction addresses a register beyond this.
    pub reg_count: u8,
    /// Index into `Program::types`.
    pub ret_type: u32,
    /// Index of the first instruction in `Program::code`.
    pub code_off: u32,
    /// Number of instructions.
    pub code_len: u32,
}

impl FuncDef {
    pub fn param_count(&self) -> usize {
        self.params.len()
    }

    pub fn contains_pc(&self, pc: u32) -> bool {
        pc >= self.code_off && pc < self.code_off + self.code_len
    }
}

/// A capability the module needs, with the signature of the call.
///
/// The signature is in the file so that the verifier can check a `CALL_CAP`
/// without trusting whoever produced the bytecode, and so that `sic verify` can
/// report what a module may do with nothing executed.
#[derive(Debug, Clone)]
pub struct CapDecl {
    pub name: String,
    pub kind: CapKind,
    /// What the grant is limited to: an absolute path, a file path.
    pub constraints: String,
    /// The digest the file has to have, or empty for a grant that does not pin
    /// what runs.
    pub pin: String,
    /// Parameter types, as indices into `Program::types`.
    pub params: Vec<u32>,
    /// Result type, as an index into `Program::types`.
    pub ret_type: u32,
}

/// Source mapping, so a runtime error or a trace can name a line of source.
#[derive(Debug, Clone, Default)]
pub struct DebugInfo {
    pub source_name: String,
    /// `(pc, line, col)`, sorted by pc. Only the instructions that start a new
    /// position are listed.
    pub lines: Vec<(u32, u32, u32)>,
}

impl DebugInfo {
    /// The source position of an instruction, if the table has one at or before
    /// it.
    pub fn position(&self, pc: u32) -> Option<(u32, u32)> {
        match self.lines.binary_search_by_key(&pc, |e| e.0) {
            Ok(i) => Some((self.lines[i].1, self.lines[i].2)),
            Err(0) => None,
            Err(i) => Some((self.lines[i - 1].1, self.lines[i - 1].2)),
        }
    }
}
